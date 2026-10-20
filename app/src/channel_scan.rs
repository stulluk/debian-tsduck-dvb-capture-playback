//! Channel scanning shared by the CLI (`dvbhub scan`), the watch TUI and the web UI.
//!
//! A scan is a list of [`ScanTarget`]s (satellite TPs from the catalogue, UHF
//! channels, cable frequencies). Each target is tuned with
//! `tsp -I dvb … -P analyze --json -P until --seconds N -O drop`; the analyze
//! report gives the TS id and the SDT service list including the DVB
//! `service_type`, so TV, radio and data services can be told apart.
//! [`channels_from_results`] turns the results into [`SavedChannel`]s that
//! carry everything needed to tune them again (satellite, LNB, DiSEqC, tuner
//! model).

use crate::analyze::{summary_from_analyze_json, ServiceInfo, ServiceKind};
use crate::channels::{ChannelDelivery, SavedChannel};
use crate::config::TsduckPaths;
use crate::lastlog;
use crate::pipeline::CaptureParams;
use crate::satellites::Satellite;
use crate::signal::{parse_signal_line, SignalStatus};
use anyhow::Context;
use serde::Serialize;
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// One frequency to tune during a scan.
#[derive(Debug, Clone, Serialize)]
pub struct ScanTarget {
    /// Short label, e.g. "10719 V 27500" or "UHF 39 (618 MHz)".
    pub label: String,
    pub delivery: ChannelDelivery,
    /// Tuning parameters (output/duration fields are ignored).
    #[serde(skip)]
    pub params: CaptureParams,
    /// Delivery systems to try in order (DVB-T2 then DVB-T for terrestrial).
    pub delivery_systems: Vec<String>,
    pub satellite: Option<String>,
    pub tuner_model: Option<String>,
}

/// Scan tuning knobs.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Seconds to listen after lock (SDT is repeated at least every 2 s).
    pub listen_secs: u32,
    /// Seconds to wait for lock before moving on.
    pub signal_timeout_s: u32,
    pub include_scrambled: bool,
    pub include_radio: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            listen_secs: 5,
            signal_timeout_s: 5,
            include_scrambled: false,
            include_radio: true,
        }
    }
}

/// Outcome of one target.
#[derive(Debug, Clone, Serialize)]
pub struct TargetResult {
    pub index: usize,
    pub label: String,
    pub locked: bool,
    /// Delivery system that locked (terrestrial T/T2 probing).
    pub delivery_system: Option<String>,
    pub ts_id: Option<u64>,
    pub snr_db: Option<f64>,
    pub strength_db: Option<f64>,
    pub services: Vec<ServiceInfo>,
    pub error: Option<String>,
}

impl TargetResult {
    /// Number of services that would be saved with `opts`.
    pub fn wanted_count(&self, opts: &ScanOptions) -> usize {
        self.services.iter().filter(|s| wanted(s, opts)).count()
    }
}

/// Progress callback events.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ScanEvent {
    TargetStart {
        index: usize,
        total: usize,
        label: String,
    },
    TargetDone {
        total: usize,
        result: TargetResult,
    },
}

/// Satellite targets from the catalogue. `only` keeps TPs whose
/// "<freq><pol initial>" (e.g. "10719v") or bare frequency matches.
pub fn satellite_targets(
    sat: &Satellite,
    device_name: &str,
    tuner_model: Option<&str>,
    only: &[String],
) -> Vec<ScanTarget> {
    sat.transponders
        .iter()
        .filter(|tp| {
            only.is_empty()
                || only.iter().any(|o| {
                    let o = o.trim().to_ascii_lowercase();
                    let pol = tp
                        .polarity
                        .chars()
                        .next()
                        .unwrap_or('x')
                        .to_ascii_lowercase();
                    o == tp.frequency_mhz.to_string() || o == format!("{}{pol}", tp.frequency_mhz)
                })
        })
        .map(|tp| ScanTarget {
            label: format!(
                "{} {} {}",
                tp.frequency_mhz,
                tp.polarity
                    .chars()
                    .next()
                    .unwrap_or('?')
                    .to_ascii_uppercase(),
                tp.symbol_rate_ksps
            ),
            delivery: ChannelDelivery::Satellite,
            params: CaptureParams {
                device_name: Some(device_name.to_string()),
                adapter: None,
                delivery_system: tp.delivery_system.clone(),
                frequency_hz: u64::from(tp.frequency_mhz) * 1_000_000,
                polarity: Some(tp.polarity.clone()),
                symbol_rate: Some(u64::from(tp.symbol_rate_ksps) * 1_000),
                modulation: Some(tp.modulation.clone()),
                fec: Some(tp.fec.clone()),
                lnb: Some(sat.tsp_lnb()),
                satellite_number: Some(sat.diseqc),
                bandwidth_hz: None,
                guard_interval: None,
                plp: None,
                dvb_status_interval_s: None,
                signal_timeout_s: 0,
                seconds: 0,
                output: String::new(),
                live_analyze: false,
            },
            delivery_systems: vec![tp.delivery_system.clone()],
            satellite: Some(sat.name.clone()),
            tuner_model: tuner_model.map(str::to_string),
        })
        .collect()
}

/// European UHF channel → centre frequency in MHz (channels 21–69, 8 MHz raster).
pub fn uhf_channel_mhz(channel: u8) -> Option<u32> {
    (21..=69)
        .contains(&channel)
        .then(|| 474 + (u32::from(channel) - 21) * 8)
}

/// Terrestrial targets: given frequencies, or the whole UHF band 21–69.
/// Each is tried as DVB-T2 first, then DVB-T.
pub fn terrestrial_targets(
    device_name: &str,
    tuner_model: Option<&str>,
    frequencies_mhz: &[u32],
    bandwidth_mhz: u8,
) -> Vec<ScanTarget> {
    let freqs: Vec<u32> = if frequencies_mhz.is_empty() {
        (21..=69).filter_map(uhf_channel_mhz).collect()
    } else {
        frequencies_mhz.to_vec()
    };
    freqs
        .into_iter()
        .map(|mhz| {
            let uhf = (21..=69u8).find(|&c| uhf_channel_mhz(c) == Some(mhz));
            ScanTarget {
                label: match uhf {
                    Some(c) => format!("UHF {c} ({mhz} MHz)"),
                    None => format!("{mhz} MHz"),
                },
                delivery: ChannelDelivery::Terrestrial,
                params: CaptureParams {
                    device_name: Some(device_name.to_string()),
                    adapter: None,
                    delivery_system: "DVB-T2".into(),
                    frequency_hz: u64::from(mhz) * 1_000_000,
                    polarity: None,
                    symbol_rate: None,
                    modulation: None,
                    fec: None,
                    lnb: None,
                    satellite_number: None,
                    bandwidth_hz: Some(u64::from(bandwidth_mhz) * 1_000_000),
                    guard_interval: None,
                    plp: None,
                    dvb_status_interval_s: None,
                    signal_timeout_s: 0,
                    seconds: 0,
                    output: String::new(),
                    live_analyze: false,
                },
                delivery_systems: vec!["DVB-T2".into(), "DVB-T".into()],
                satellite: None,
                tuner_model: tuner_model.map(str::to_string),
            }
        })
        .collect()
}

/// Cable targets for explicit frequencies (cable has no standard raster).
pub fn cable_targets(
    device_name: &str,
    tuner_model: Option<&str>,
    frequencies_mhz: &[u32],
    symbol_rate_ksps: u32,
    modulation: &str,
) -> Vec<ScanTarget> {
    frequencies_mhz
        .iter()
        .map(|&mhz| ScanTarget {
            label: format!("{mhz} MHz {symbol_rate_ksps} {modulation}"),
            delivery: ChannelDelivery::Cable,
            params: CaptureParams {
                device_name: Some(device_name.to_string()),
                adapter: None,
                delivery_system: "DVB-C".into(),
                frequency_hz: u64::from(mhz) * 1_000_000,
                polarity: None,
                symbol_rate: Some(u64::from(symbol_rate_ksps) * 1_000),
                modulation: Some(modulation.to_string()),
                fec: None,
                lnb: None,
                satellite_number: None,
                bandwidth_hz: None,
                guard_interval: None,
                plp: None,
                dvb_status_interval_s: None,
                signal_timeout_s: 0,
                seconds: 0,
                output: String::new(),
                live_analyze: false,
            },
            delivery_systems: vec!["DVB-C".into()],
            satellite: None,
            tuner_model: tuner_model.map(str::to_string),
        })
        .collect()
}

/// `tsp` arguments that tune `params` with `delsys`, listen for `listen_secs`
/// and print the analyze report as JSON on stdout.
pub fn scan_tsp_args(params: &CaptureParams, delsys: &str, opts: &ScanOptions) -> Vec<String> {
    let mut p = params.clone();
    p.delivery_system = delsys.to_string();
    p.signal_timeout_s = opts.signal_timeout_s;
    p.seconds = 0;
    p.live_analyze = false;
    p.dvb_status_interval_s = None;
    let mut args = p.to_tsp_args();
    if let Some(i) = args.iter().position(|a| a == "-O") {
        args.truncate(i);
    }
    args.extend(
        ["-P", "analyze", "--json", "-P", "until", "--seconds"]
            .iter()
            .map(|s| s.to_string()),
    );
    args.push(opts.listen_secs.to_string());
    args.extend(["-O".to_string(), "drop".to_string()]);
    args
}

/// Runs the scan, calling `on_event` for progress. Stops early when `cancel` is set.
pub fn scan(
    paths: &TsduckPaths,
    targets: &[ScanTarget],
    opts: &ScanOptions,
    cancel: &AtomicBool,
    mut on_event: impl FnMut(ScanEvent),
) -> Vec<TargetResult> {
    let total = targets.len();
    let mut results = Vec::with_capacity(total);
    for (index, target) in targets.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        on_event(ScanEvent::TargetStart {
            index,
            total,
            label: target.label.clone(),
        });
        let result = scan_target(paths, index, target, opts, cancel);
        lastlog::event(
            "scan",
            &format!(
                "{} locked={} delsys={:?} services={} error={:?}",
                result.label,
                result.locked,
                result.delivery_system,
                result.services.len(),
                result.error
            ),
        );
        on_event(ScanEvent::TargetDone {
            total,
            result: result.clone(),
        });
        results.push(result);
    }
    results
}

fn scan_target(
    paths: &TsduckPaths,
    index: usize,
    target: &ScanTarget,
    opts: &ScanOptions,
    cancel: &AtomicBool,
) -> TargetResult {
    let mut last = TargetResult {
        index,
        label: target.label.clone(),
        locked: false,
        delivery_system: None,
        ts_id: None,
        snr_db: None,
        strength_db: None,
        services: Vec::new(),
        error: None,
    };
    for delsys in &target.delivery_systems {
        let args = scan_tsp_args(&target.params, delsys, opts);
        match run_scan_tsp(paths, &args, opts, cancel) {
            Ok(run) => {
                last.locked = run.signal.as_ref().is_some_and(|s| s.locked);
                last.snr_db = run.signal.as_ref().and_then(|s| s.snr_db);
                last.strength_db = run.signal.as_ref().and_then(|s| s.strength_db);
                last.error = run.error;
                if let Some(summary) = run.summary {
                    last.ts_id = summary.ts_id;
                    last.services = summary.services;
                }
                if last.locked {
                    last.delivery_system = Some(delsys.clone());
                    break;
                }
            }
            Err(e) => last.error = Some(format!("{e:#}")),
        }
        if cancel.load(Ordering::SeqCst) {
            break;
        }
    }
    last
}

struct ScanRun {
    signal: Option<SignalStatus>,
    summary: Option<crate::analyze::StreamSummary>,
    error: Option<String>,
}

fn run_scan_tsp(
    paths: &TsduckPaths,
    args: &[String],
    opts: &ScanOptions,
    cancel: &AtomicBool,
) -> anyhow::Result<ScanRun> {
    let bin = paths.tool("tsp");
    let mut child = Command::new(&bin)
        .arg("--verbose")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start {}", bin.display()))?;
    let stdout = read_all(child.stdout.take().context("tsp stdout")?);
    let signal = Arc::new(Mutex::new(None::<SignalStatus>));
    let error = Arc::new(Mutex::new(None::<String>));
    let stderr = {
        let (signal, error) = (Arc::clone(&signal), Arc::clone(&error));
        let err = child.stderr.take().context("tsp stderr")?;
        thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                if let Some(sig) = parse_signal_line(&line) {
                    if let Ok(mut g) = signal.lock() {
                        *g = Some(sig);
                    }
                } else if line.contains("Error") {
                    if let Ok(mut g) = error.lock() {
                        *g = Some(line.trim().to_string());
                    }
                }
            }
        })
    };

    // Lock wait + listen window + slack; never let a stuck tuner hang the scan.
    let deadline = Instant::now()
        + Duration::from_secs(u64::from(opts.signal_timeout_s + opts.listen_secs) + 10);
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if cancel.load(Ordering::SeqCst) || Instant::now() > deadline {
            unsafe {
                libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
            }
            let _ = child.wait();
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    let _ = stderr.join();
    let json = stdout.join().unwrap_or_default();
    let signal = signal.lock().ok().and_then(|g| g.clone());
    let error = error.lock().ok().and_then(|g| g.clone());
    Ok(ScanRun {
        signal,
        summary: summary_from_analyze_json(json.trim()),
        error,
    })
}

fn read_all(mut r: impl Read + Send + 'static) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut s = String::new();
        let _ = r.read_to_string(&mut s);
        s
    })
}

/// True when a service should be saved as a channel. TV needs a video stream
/// and radio an audio stream: data services are often labelled "TV" in the
/// SDT, and some FTA entries are listed while off air (no streams at all).
/// Without a PID table (older data) a PCR / component count is used instead.
fn wanted(s: &ServiceInfo, opts: &ScanOptions) -> bool {
    let on_air = s.components.unwrap_or(1) > 0;
    let kind_ok = match s.kind() {
        ServiceKind::Tv => s.has_video.unwrap_or(s.pcr_pid.is_some()),
        ServiceKind::Radio => opts.include_radio && s.has_audio.unwrap_or(on_air),
        ServiceKind::Data => false,
        // No/unknown type: keep only services that carry a clock (playable).
        ServiceKind::Unknown => s.has_video.unwrap_or(s.pcr_pid.is_some()),
    };
    kind_ok && (opts.include_scrambled || !s.scrambled) && !s.name.trim().is_empty()
}

/// Converts scan results into channels (scan order = target order, then SDT order).
pub fn channels_from_results(
    results: &[TargetResult],
    targets: &[ScanTarget],
    opts: &ScanOptions,
) -> Vec<SavedChannel> {
    let mut out = Vec::new();
    for r in results.iter().filter(|r| r.locked) {
        let Some(t) = targets.get(r.index) else {
            continue;
        };
        let p = &t.params;
        for s in r.services.iter().filter(|s| wanted(s, opts)) {
            out.push(SavedChannel {
                delivery: t.delivery.clone(),
                name: s.name.trim().to_string(),
                service_id: s.id,
                provider: s.provider.clone(),
                scrambled: s.scrambled,
                device_name: p.device_name.clone(),
                tuner_model: t.tuner_model.clone(),
                satellite: t.satellite.clone(),
                lnb: p.lnb.clone(),
                delivery_system: r
                    .delivery_system
                    .clone()
                    .or_else(|| Some(p.delivery_system.clone())),
                frequency_mhz: Some((p.frequency_hz / 1_000_000) as u32),
                polarity: p.polarity.clone(),
                symbol_rate_ksps: p.symbol_rate.map(|s| (s / 1_000) as u32),
                modulation: p.modulation.clone(),
                fec: p.fec.clone(),
                satellite_number: p.satellite_number,
                bandwidth_mhz: p.bandwidth_hz.map(|b| (b / 1_000_000) as u8),
                guard_interval: p.guard_interval.clone(),
                plp: p.plp,
                ..Default::default()
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::satellites::SatTransponder;

    fn hotbird() -> Satellite {
        Satellite {
            name: "Hot Bird 13E".into(),
            longitude: "13.0E".into(),
            lnb: "universal".into(),
            diseqc: 0,
            transponders: vec![
                SatTransponder {
                    frequency_mhz: 10719,
                    polarity: "vertical".into(),
                    symbol_rate_ksps: 27500,
                    modulation: "8-PSK".into(),
                    fec: "5/6".into(),
                    delivery_system: "DVB-S2".into(),
                    name: String::new(),
                },
                SatTransponder {
                    frequency_mhz: 10930,
                    polarity: "horizontal".into(),
                    symbol_rate_ksps: 30000,
                    modulation: "8-PSK".into(),
                    fec: "2/3".into(),
                    delivery_system: "DVB-S2".into(),
                    name: String::new(),
                },
            ],
        }
    }

    #[test]
    fn satellite_targets_filter_and_lnb() {
        let t = satellite_targets(
            &hotbird(),
            "/dev/dvb/adapter0:1",
            Some("TBS5530"),
            &["10930h".into()],
        );
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].label, "10930 H 30000");
        assert_eq!(t[0].params.lnb.as_deref(), Some("Extended"));
        let all = satellite_targets(&hotbird(), "x", None, &[]);
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn uhf_raster() {
        assert_eq!(uhf_channel_mhz(21), Some(474));
        assert_eq!(uhf_channel_mhz(39), Some(618));
        assert_eq!(uhf_channel_mhz(69), Some(858));
        assert_eq!(uhf_channel_mhz(70), None);
        let t = terrestrial_targets("x", None, &[618], 8);
        assert_eq!(t[0].label, "UHF 39 (618 MHz)");
        assert_eq!(t[0].delivery_systems, vec!["DVB-T2", "DVB-T"]);
    }

    #[test]
    fn scan_args_listen_and_drop() {
        let t = satellite_targets(&hotbird(), "/dev/dvb/adapter0:1", None, &["10719".into()]);
        let args = scan_tsp_args(&t[0].params, "DVB-S2", &ScanOptions::default());
        let joined = args.join(" ");
        assert!(joined.contains("--lnb Extended"));
        assert!(joined.contains("-P analyze --json -P until --seconds 5 -O drop"));
        assert!(joined.contains("--signal-timeout 5"));
    }

    #[test]
    fn channels_keep_fta_tv_and_radio_only() {
        let targets = satellite_targets(&hotbird(), "/dev/dvb/adapter0:1", Some("TBS5530"), &[]);
        let svc = |id, name: &str, ty, scr| ServiceInfo {
            id,
            name: name.into(),
            provider: "P".into(),
            scrambled: scr,
            service_type: Some(ty),
            pcr_pid: (ty != 0x0C).then_some(100),
            components: Some(3),
            has_video: Some(ty == 0x19),
            has_audio: Some(ty != 0x0C),
        };
        let results = vec![TargetResult {
            index: 0,
            label: targets[0].label.clone(),
            locked: true,
            delivery_system: Some("DVB-S2".into()),
            ts_id: Some(11000),
            snr_db: None,
            strength_db: None,
            services: vec![
                svc(4409, "TVP INFO HD", 0x19, false),
                svc(4401, "CANAL+ 360", 0x19, true),
                svc(4495, "DATA", 0x0C, false),
                // Labelled TV with a PCR but carrying only data streams.
                ServiceInfo {
                    id: 16999,
                    name: "Eutelsat Data 1".into(),
                    service_type: Some(0x01),
                    pcr_pid: Some(200),
                    components: Some(4),
                    has_video: Some(false),
                    has_audio: Some(false),
                    ..Default::default()
                },
                svc(4500, "Radio", 0x02, false),
            ],
            error: None,
        }];
        let mut results = results;
        // Listed in the SDT as FTA TV but off air: no PCR, no components.
        results[0].services.push(ServiceInfo {
            id: 4415,
            name: "Hustler HD".into(),
            service_type: Some(0x19),
            components: Some(0),
            ..Default::default()
        });
        let ch = channels_from_results(&results, &targets, &ScanOptions::default());
        let names: Vec<_> = ch.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["TVP INFO HD", "Radio"]);
        assert_eq!(ch[0].satellite.as_deref(), Some("Hot Bird 13E"));
        assert_eq!(ch[0].lnb.as_deref(), Some("Extended"));
        assert_eq!(ch[0].tuner_model.as_deref(), Some("TBS5530"));
        assert_eq!(ch[0].frequency_mhz, Some(10719));
    }
}

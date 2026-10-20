//! Sequential satellite capture into per-transponder folders.
//!
//! Each capture is written under `<root>/<freq><pol><sr>/` as timestamped files:
//! `<sat>_<mhz>_<pol>_<sr>_<YYYYMMDD_HHMMSS>.ts` plus matching `*_services.json`.

use crate::analyze::{best_capture_summary, CaptureServicesRecord, TuneRecord};
use crate::config::TsduckPaths;
use crate::pipeline::CaptureParams;
use crate::progress::{self, CaptureEvent, CaptureRun};
use crate::satellites::{Satellite, SatTransponder};
use anyhow::Context;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{atomic::AtomicBool, Arc};

/// Slug for satellite directory names (`Turksat 42E` → `turksat42e`).
pub fn satellite_slug(name: &str) -> String {
    let s: String = name
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    if s.is_empty() {
        "sat".into()
    } else {
        s
    }
}

/// Directory name for one transponder (`11054v30000`).
pub fn tp_dir_name(tp: &SatTransponder) -> String {
    let pol = tp
        .polarity
        .chars()
        .next()
        .unwrap_or('x')
        .to_ascii_lowercase();
    format!("{}{}{}", tp.frequency_mhz, pol, tp.symbol_rate_ksps)
}

/// Base file stem: `<sat>_<mhz>_<pol>_<sr>_<YYYYMMDD_HHMMSS>`.
pub fn capture_base_name(sat: &Satellite, tp: &SatTransponder) -> String {
    let slug = satellite_slug(&sat.name);
    let pol = tp
        .polarity
        .chars()
        .next()
        .unwrap_or('x')
        .to_ascii_lowercase();
    let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    format!(
        "{}_{}_{}_{}_{}",
        slug, tp.frequency_mhz, pol, tp.symbol_rate_ksps, stamp
    )
}

/// Transport-stream file name for one capture.
pub fn capture_ts_filename(sat: &Satellite, tp: &SatTransponder) -> String {
    format!("{}.ts", capture_base_name(sat, tp))
}

/// Sidecar metadata file name for one capture.
pub fn capture_services_filename(sat: &Satellite, tp: &SatTransponder) -> String {
    format!("{}_services.json", capture_base_name(sat, tp))
}

/// Options for a multi-TP satellite capture run.
pub struct SatCaptureOptions {
    pub satellite: Satellite,
    pub device_name: String,
    pub seconds: u32,
    pub signal_timeout_s: u32,
    pub transponders: Vec<SatTransponder>,
    pub output_root: PathBuf,
    pub quiet: bool,
    pub max_tps: Option<usize>,
    /// Skip transponders that already have a valid `.ts` in their folder.
    pub skip_existing: bool,
}

/// Result of capturing one transponder folder.
#[derive(Debug)]
pub struct TpCaptureOutcome {
    pub run: CaptureRun,
    pub ts_path: PathBuf,
    pub services_path: PathBuf,
}

/// Outcome of a full (or partial) satellite capture pass.
#[derive(Debug)]
pub struct SatCaptureReport {
    pub outcomes: Vec<TpCaptureOutcome>,
    pub skipped: usize,
}

/// Captures one transponder into `output_root/<tp_dir>/`.
pub fn capture_transponder(
    paths: &TsduckPaths,
    sat: &Satellite,
    device: &str,
    tp: &SatTransponder,
    seconds: u32,
    signal_timeout_s: u32,
    output_root: &Path,
    quiet: bool,
    events: Option<Sender<CaptureEvent>>,
    cancel: Option<Arc<AtomicBool>>,
) -> anyhow::Result<TpCaptureOutcome> {
    let tp_dir = output_root.join(tp_dir_name(tp));
    std::fs::create_dir_all(&tp_dir)
        .with_context(|| format!("cannot create {}", tp_dir.display()))?;
    let ts_path = tp_dir.join(capture_ts_filename(sat, tp));
    let services_path = tp_dir.join(capture_services_filename(sat, tp));

    let params = capture_params_from_sat_tp(
        sat,
        device,
        tp,
        seconds,
        signal_timeout_s,
        ts_path.to_string_lossy().into_owned(),
    );
    let args = params.to_tsp_args();
    let mut run = if let Some(tx) = events {
        progress::run_capture_with_events(paths, &args, seconds, signal_timeout_s, tx, cancel)?
    } else if quiet {
        progress::run_capture_quiet(paths, &args, seconds, signal_timeout_s, cancel)?
    } else {
        progress::run_capture_progress(paths, &args, seconds, signal_timeout_s)?
    };

    let tune = tune_record_from_tp(sat, tp);
    let summary = best_capture_summary(
        paths,
        &ts_path,
        run.last_summary.as_ref(),
        run.success && run.ever_locked,
    );
    if let Some(summary) = summary {
        let mut record = CaptureServicesRecord::from_summary(&summary, &tune);
        record.signal_stats = run.signal_stats.clone();
        record.write_file(&services_path)?;
        run.last_summary = Some(summary);
    }

    Ok(TpCaptureOutcome {
        run,
        ts_path,
        services_path,
    })
}

/// Runs capture for every transponder in `opts.transponders`.
pub fn run_sat_capture(
    paths: &TsduckPaths,
    opts: SatCaptureOptions,
) -> anyhow::Result<SatCaptureReport> {
    std::fs::create_dir_all(&opts.output_root).with_context(|| {
        format!(
            "cannot create output dir {}",
            opts.output_root.display()
        )
    })?;

    let total = opts.transponders.len();
    let limit = opts.max_tps.unwrap_or(total).min(total);
    let mut outcomes = Vec::new();
    let mut skipped = 0usize;
    let mut resumed = 0usize;

    for (idx, tp) in opts.transponders.iter().take(limit).enumerate() {
        let tp_dir = opts.output_root.join(tp_dir_name(tp));
        if opts.skip_existing {
            if let Some((ts_path, services_path)) = find_existing_capture(&tp_dir) {
                resumed += 1;
                if !opts.quiet {
                    eprintln!(
                        "[dvbhub] SAT capture {}/{}: {} MHz {} — resume (existing {})",
                        idx + 1,
                        limit,
                        tp.frequency_mhz,
                        tp.polarity,
                        ts_path.file_name().and_then(|n| n.to_str()).unwrap_or("capture.ts")
                    );
                }
                outcomes.push(TpCaptureOutcome {
                    run: CaptureRun {
                        success: true,
                        ever_locked: true,
                        services_found: services_path.is_file(),
                        killed_by_watchdog: false,
                        last_signal: None,
                        last_summary: None,
                        signal_stats: None,
                    },
                    ts_path,
                    services_path,
                });
                continue;
            }
        }
        if !opts.quiet {
            eprintln!(
                "[dvbhub] SAT capture {}/{}: {} MHz {} {} → {}/{}",
                idx + 1,
                limit,
                tp.frequency_mhz,
                tp.polarity,
                tp.delivery_system,
                opts.output_root.display(),
                tp_dir_name(tp)
            );
        }
        match capture_transponder(
            paths,
            &opts.satellite,
            &opts.device_name,
            tp,
            opts.seconds,
            opts.signal_timeout_s,
            &opts.output_root,
            opts.quiet,
            None,
            None,
        ) {
            Ok(out) => {
                if out.run.killed_by_watchdog {
                    skipped += 1;
                    if !opts.quiet {
                        eprintln!(
                            "[dvbhub]   ! TP {} MHz {} — killed by watchdog",
                            tp.frequency_mhz, tp.polarity
                        );
                    }
                } else if !out.run.success || !out.run.ever_locked {
                    skipped += 1;
                    if !opts.quiet {
                        eprintln!(
                            "[dvbhub]   ! TP {} MHz {} — no lock or tsp error",
                            tp.frequency_mhz, tp.polarity
                        );
                    }
                }
                outcomes.push(out);
            }
            Err(e) => {
                skipped += 1;
                if !opts.quiet {
                    eprintln!("[dvbhub]   ! TP failed: {e}");
                }
            }
        }
    }

    if !opts.quiet {
        eprintln!(
            "[dvbhub] SAT capture done: {} TP folder(s), {} skipped/failed{}",
            outcomes.len(),
            skipped,
            if resumed > 0 {
                format!(", {resumed} resumed")
            } else {
                String::new()
            }
        );
    }

    Ok(SatCaptureReport {
        outcomes,
        skipped,
    })
}

/// The `tsp` command line a catalogue capture of `tp` would run (for `--dry-run`).
pub fn dry_run_command(
    sat: &Satellite,
    device: &str,
    tp: &SatTransponder,
    seconds: u32,
    signal_timeout_s: u32,
    output_root: &Path,
) -> String {
    let ts_path = output_root
        .join(tp_dir_name(tp))
        .join(capture_ts_filename(sat, tp));
    let params = capture_params_from_sat_tp(
        sat,
        device,
        tp,
        seconds,
        signal_timeout_s,
        ts_path.display().to_string(),
    );
    crate::tsduck::display_command("tsp", &params.to_tsp_args())
}

fn capture_params_from_sat_tp(
    sat: &Satellite,
    device: &str,
    tp: &SatTransponder,
    seconds: u32,
    signal_timeout_s: u32,
    output: String,
) -> CaptureParams {
    CaptureParams {
        device_name: Some(device.to_string()),
        adapter: None,
        delivery_system: tp.delivery_system.clone(),
        frequency_hz: tp.frequency_mhz as u64 * 1_000_000,
        polarity: Some(tp.polarity.clone()),
        symbol_rate: Some(tp.symbol_rate_ksps as u64 * 1_000),
        modulation: Some(tp.modulation.clone()),
        fec: Some(tp.fec.clone()),
        lnb: Some(sat.tsp_lnb()),
        satellite_number: Some(sat.diseqc),
        bandwidth_hz: None,
        guard_interval: None,
        plp: None,
        dvb_status_interval_s: None,
        signal_timeout_s,
        seconds,
        output,
        live_analyze: true,
    }
}

fn tune_record_from_tp(sat: &Satellite, tp: &SatTransponder) -> TuneRecord {
    TuneRecord {
        satellite: Some(sat.name.clone()),
        lnb: Some(sat.tsp_lnb()),
        satellite_number: Some(sat.diseqc),
        frequency_mhz: tp.frequency_mhz,
        polarity: tp.polarity.clone(),
        symbol_rate_ksps: tp.symbol_rate_ksps,
        delivery_system: tp.delivery_system.clone(),
        modulation: tp.modulation.clone(),
        fec: tp.fec.clone(),
        ..Default::default()
    }
}

const MIN_CAPTURE_BYTES: u64 = 500_000;

/// Returns the largest valid `.ts` in `tp_dir` and a matching `*_services.json` if present.
fn find_existing_capture(tp_dir: &Path) -> Option<(PathBuf, PathBuf)> {
    let mut best: Option<(PathBuf, u64)> = None;
    let entries = std::fs::read_dir(tp_dir).ok()?;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("ts") {
            continue;
        }
        let len = entry.metadata().ok()?.len();
        if len < MIN_CAPTURE_BYTES {
            continue;
        }
        if best.as_ref().is_none_or(|(_, n)| len > *n) {
            best = Some((path, len));
        }
    }
    let (ts_path, _) = best?;
    let stem = ts_path.file_stem()?.to_str()?;
    let services_path = tp_dir.join(format!("{stem}_services.json"));
    let services_path = if services_path.is_file() {
        services_path
    } else {
        PathBuf::new()
    };
    Some((ts_path, services_path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::satellites::SatelliteDb;

    #[test]
    fn capture_base_name_contains_sat_freq_pol_sr() {
        let db = SatelliteDb::seed();
        let sat = &db.satellites[0];
        let tp = sat
            .transponders
            .iter()
            .find(|t| t.frequency_mhz == 11054)
            .expect("seed TP 11054");
        let name = capture_base_name(sat, tp);
        assert!(name.starts_with("turksat42e_"));
        assert!(name.contains("_11054_"));
        assert!(name.contains("_v_"));
        assert!(name.contains("_30000_"));
    }

    #[test]
    fn capture_filenames_use_base_stem() {
        let db = SatelliteDb::seed();
        let sat = &db.satellites[0];
        let tp = sat
            .transponders
            .iter()
            .find(|t| t.frequency_mhz == 11054)
            .expect("seed TP 11054");
        let base = capture_base_name(sat, tp);
        assert_eq!(capture_ts_filename(sat, tp), format!("{base}.ts"));
        assert_eq!(
            capture_services_filename(sat, tp),
            format!("{base}_services.json")
        );
    }
}

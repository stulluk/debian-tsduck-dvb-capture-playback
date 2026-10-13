//! Frontend signal statistics sampled during a capture.
//!
//! TSDuck's periodic `dvbstatus` report only gives BER as an integer percentage,
//! which is useless for DVB-S2 links whose post-LDPC BER lives around 1e-3..1e-6.
//! While `tsp` owns the frontend, `dvb-fe-tool -m` (Debian package `dvb-tools`)
//! can open it read-only and print the DVBv5 statistics once per second:
//!
//! ```text
//! Lock   (0x1f) Signal= -39.05dBm C/N= 10.79dB postBER= 26.2x10^-3
//! ```
//!
//! [`FeMonitor`] runs that tool next to `tsp`, collects the locked samples and
//! reduces them to [`SignalStats`] (C/N min/avg/max, level, post-BER).

use crate::tools::find_executable_on_path;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

/// External tool used for sampling (from the `dvb-tools` package).
pub const FE_TOOL: &str = "dvb-fe-tool";

/// One `dvb-fe-tool -m` sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FeSample {
    pub locked: bool,
    pub level_dbm: Option<f64>,
    pub cn_db: Option<f64>,
    pub post_ber: Option<f64>,
}

/// Aggregated statistics over the locked samples of one capture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignalStats {
    pub samples: usize,
    pub cn_min_db: Option<f64>,
    pub cn_avg_db: Option<f64>,
    pub cn_max_db: Option<f64>,
    pub level_avg_dbm: Option<f64>,
    pub post_ber_avg: Option<f64>,
    pub post_ber_max: Option<f64>,
}

impl SignalStats {
    /// Reduces samples to statistics; only locked samples count.
    ///
    /// With three or more locked samples the first and last are dropped: right
    /// after lock the demodulator's C/N estimate is still settling (seen as a
    /// single 3 dB reading on a 11-12 dB link), and the last one can overlap
    /// `tsp` closing the frontend.
    pub fn from_samples(samples: &[FeSample]) -> Option<Self> {
        let mut locked: Vec<&FeSample> = samples.iter().filter(|s| s.locked).collect();
        if locked.is_empty() {
            return None;
        }
        if locked.len() >= 3 {
            locked = locked[1..locked.len() - 1].to_vec();
        }
        let cn: Vec<f64> = locked.iter().filter_map(|s| s.cn_db).collect();
        let level: Vec<f64> = locked.iter().filter_map(|s| s.level_dbm).collect();
        let ber: Vec<f64> = locked.iter().filter_map(|s| s.post_ber).collect();
        Some(Self {
            samples: locked.len(),
            cn_min_db: cn.iter().copied().reduce(f64::min),
            cn_avg_db: mean(&cn),
            cn_max_db: cn.iter().copied().reduce(f64::max),
            level_avg_dbm: mean(&level),
            post_ber_avg: mean(&ber),
            post_ber_max: ber.iter().copied().reduce(f64::max),
        })
    }

    /// One-line summary for CLI output and the TUI log pane.
    pub fn summary(&self) -> String {
        let db = |v: Option<f64>| v.map(|x| format!("{x:.2}")).unwrap_or_else(|| "n/a".into());
        let sci = |v: Option<f64>| {
            v.map(|x| format!("{x:.1e}"))
                .unwrap_or_else(|| "n/a".into())
        };
        format!(
            "Signal stats ({} samples): C/N min/avg/max {} / {} / {} dB, level avg {} dBm, \
       postBER avg {} max {}",
            self.samples,
            db(self.cn_min_db),
            db(self.cn_avg_db),
            db(self.cn_max_db),
            self.level_avg_dbm
                .map(|x| format!("{x:.1}"))
                .unwrap_or_else(|| "n/a".into()),
            sci(self.post_ber_avg),
            sci(self.post_ber_max),
        )
    }
}

fn mean(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        None
    } else {
        Some(values.iter().sum::<f64>() / values.len() as f64)
    }
}

/// Parses a `dvb-fe-tool -m` status line; ignores `Layer A:` and other lines.
pub fn parse_fe_tool_line(line: &str) -> Option<FeSample> {
    let line = line.trim();
    if line.starts_with("Layer") || !line.contains("Signal=") {
        return None;
    }
    let locked = line.starts_with("Lock");
    Some(FeSample {
        locked,
        level_dbm: value_after(line, "Signal=").and_then(|v| v.strip_suffix("dBm")?.parse().ok()),
        cn_db: value_after(line, "C/N=").and_then(|v| v.strip_suffix("dB")?.parse().ok()),
        post_ber: value_after(line, "postBER=").and_then(parse_ber),
    })
}

fn value_after<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split(key).nth(1)?.split_whitespace().next()
}

/// Parses `26.2x10^-3`, `288x10^-6` or `0` into a float.
fn parse_ber(text: &str) -> Option<f64> {
    match text.split_once("x10^") {
        Some((mantissa, exp)) => {
            Some(mantissa.parse::<f64>().ok()? * 10f64.powi(exp.parse::<i32>().ok()?))
        }
        None => text.parse().ok(),
    }
}

/// Extracts `(adapter, frontend)` from `tsp -I dvb` arguments
/// (`--device-name /dev/dvb/adapterA[:F[:M[:V]]]` or `--adapter A`).
pub fn adapter_frontend_from_args(args: &[String]) -> Option<(u32, u32)> {
    let value_of = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
    };
    if let Some(dev) = value_of("--device-name") {
        let rest = dev.rsplit('/').next()?.strip_prefix("adapter")?;
        let mut parts = rest.split(':');
        let adapter = parts.next()?.parse().ok()?;
        let frontend = parts.next().map_or(Some(0), |f| f.parse().ok())?;
        return Some((adapter, frontend));
    }
    value_of("--adapter")
        .and_then(|a| a.parse().ok())
        .map(|a| (a, 0))
}

/// Background `dvb-fe-tool -m` sampler for one frontend.
pub struct FeMonitor {
    child: Child,
    reader: Option<JoinHandle<()>>,
    samples: Arc<Mutex<Vec<FeSample>>>,
}

impl FeMonitor {
    /// True when `dvb-fe-tool` is available on `$PATH`.
    pub fn available() -> bool {
        find_executable_on_path(FE_TOOL).is_some()
    }

    /// Starts sampling `adapter`/`frontend`. The frontend is opened read-only, so
    /// it does not disturb the `tsp` process that tuned it. The statistics are
    /// printed on stderr (unbuffered), so every sample reaches us before stop.
    pub fn start(adapter: u32, frontend: u32) -> anyhow::Result<Self> {
        let mut child = Command::new(FE_TOOL)
            .args([
                "-a",
                &adapter.to_string(),
                "-f",
                &frontend.to_string(),
                "-m",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        let stderr = child.stderr.take().expect("stderr piped");
        let samples = Arc::new(Mutex::new(Vec::new()));
        let store = samples.clone();
        let reader = thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if let Some(sample) = parse_fe_tool_line(&line) {
                    if let Ok(mut guard) = store.lock() {
                        guard.push(sample);
                    }
                }
            }
        });
        Ok(Self {
            child,
            reader: Some(reader),
            samples,
        })
    }

    /// Stops the sampler and returns the aggregated statistics.
    pub fn stop(mut self) -> Option<SignalStats> {
        // SIGTERM: dvb-fe-tool exits cleanly (its attempt to reset the LNB voltage
        // is refused on the read-only frontend, so tsp's tuning is untouched).
        unsafe {
            libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM);
        }
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        let samples = self.samples.lock().ok()?.clone();
        SignalStats::from_samples(&samples)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_locked_sample() {
        let s =
            parse_fe_tool_line("Lock   (0x1f) Signal= -39.05dBm C/N= 10.79dB postBER= 26.2x10^-3")
                .unwrap();
        assert!(s.locked);
        assert_eq!(s.level_dbm, Some(-39.05));
        assert_eq!(s.cn_db, Some(10.79));
        assert!((s.post_ber.unwrap() - 0.0262).abs() < 1e-9);
    }

    #[test]
    fn ignores_layer_lines_and_parses_zero_ber() {
        assert!(parse_fe_tool_line("\t  Layer A: Signal= 90.09% C/N= 53.55%").is_none());
        let s =
            parse_fe_tool_line("Lock   (0x1f) Signal= -38.20dBm C/N= 11.44dB postBER= 0").unwrap();
        assert_eq!(s.post_ber, Some(0.0));
    }

    #[test]
    fn stats_use_locked_samples_only() {
        let samples = [
            FeSample {
                locked: false,
                level_dbm: Some(-60.0),
                cn_db: Some(0.0),
                post_ber: Some(1.0),
            },
            FeSample {
                locked: true,
                level_dbm: Some(-39.0),
                cn_db: Some(10.0),
                post_ber: Some(1e-3),
            },
            FeSample {
                locked: true,
                level_dbm: Some(-41.0),
                cn_db: Some(12.0),
                post_ber: Some(0.0),
            },
        ];
        let st = SignalStats::from_samples(&samples).unwrap();
        assert_eq!(st.samples, 2);
        assert_eq!(st.cn_min_db, Some(10.0));
        assert_eq!(st.cn_avg_db, Some(11.0));
        assert_eq!(st.cn_max_db, Some(12.0));
        assert_eq!(st.level_avg_dbm, Some(-40.0));
        assert_eq!(st.post_ber_max, Some(1e-3));
    }

    #[test]
    fn adapter_frontend_parsing() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            adapter_frontend_from_args(&args(&["--device-name", "/dev/dvb/adapter0:1"])),
            Some((0, 1))
        );
        assert_eq!(
            adapter_frontend_from_args(&args(&["--device-name", "/dev/dvb/adapter2:1:0:0"])),
            Some((2, 1))
        );
        assert_eq!(
            adapter_frontend_from_args(&args(&["--device-name", "/dev/dvb/adapter1"])),
            Some((1, 0))
        );
        assert_eq!(
            adapter_frontend_from_args(&args(&["--adapter", "3"])),
            Some((3, 0))
        );
        assert_eq!(adapter_frontend_from_args(&args(&["-I", "file"])), None);
    }

    #[test]
    fn stats_drop_first_and_last_locked_sample() {
        let at = |cn: f64| FeSample {
            locked: true,
            level_dbm: Some(-40.0),
            cn_db: Some(cn),
            post_ber: Some(0.0),
        };
        let st = SignalStats::from_samples(&[at(3.0), at(11.0), at(12.0), at(5.0)]).unwrap();
        assert_eq!(st.samples, 2);
        assert_eq!(st.cn_min_db, Some(11.0));
        assert_eq!(st.cn_max_db, Some(12.0));
    }
}

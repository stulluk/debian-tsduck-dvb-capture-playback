//! Single DVB-C multiplex capture to timestamped `.ts` + services JSON.

use crate::analyze::{best_capture_summary, CaptureServicesRecord, TuneRecord};
use crate::cable::{capture_base_name, freq_dir_name, CableMux};
use crate::config::TsduckPaths;
use crate::progress::{self, CaptureEvent, CaptureRun};
use anyhow::Context;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, atomic::AtomicBool};

#[derive(Debug)]
pub struct CableCaptureOutcome {
    pub run: CaptureRun,
    pub ts_path: PathBuf,
    pub services_path: PathBuf,
}

pub fn capture_mux(
    paths: &TsduckPaths,
    device: &str,
    mux: &CableMux,
    seconds: u32,
    signal_timeout_s: u32,
    output_root: &Path,
    quiet: bool,
    events: Option<Sender<CaptureEvent>>,
    cancel: Option<Arc<AtomicBool>>,
) -> anyhow::Result<CableCaptureOutcome> {
    let mux_dir = output_root.join(freq_dir_name(mux.frequency_mhz));
    std::fs::create_dir_all(&mux_dir)
        .with_context(|| format!("cannot create {}", mux_dir.display()))?;
    let stem = capture_base_name(mux);
    let ts_path = mux_dir.join(format!("{stem}.ts"));
    let services_path = mux_dir.join(format!("{stem}_services.json"));

    let params = mux.to_capture_params(
        device,
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

    let tune = tune_record_from_mux(mux);
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

    Ok(CableCaptureOutcome {
        run,
        ts_path,
        services_path,
    })
}

fn tune_record_from_mux(mux: &CableMux) -> TuneRecord {
    TuneRecord {
        frequency_mhz: mux.frequency_mhz,
        polarity: String::new(),
        symbol_rate_ksps: mux.symbol_rate_ksps,
        delivery_system: mux.delivery_system.clone(),
        modulation: mux.modulation.clone(),
        fec: mux.fec.clone().unwrap_or_else(|| "auto".into()),
        ..Default::default()
    }
}

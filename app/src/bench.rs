//! Bench playout transport streams under `<capture_dir>/bench/`.

use crate::analyze::{CaptureServicesRecord, TuneRecord};
use std::path::{Path, PathBuf};

pub const BENCH_SUBDIR: &str = "bench";

pub const CABLE_TS: &str = "vectra_722_dvbc.ts";
pub const TERR_TS: &str = "istanbul_618_dvbt2.ts";

/// `<home>/dvbhub-captures/bench` (or configured capture dir + bench).
pub fn bench_dir(capture_dir: &Path) -> PathBuf {
    capture_dir.join(BENCH_SUBDIR)
}

pub fn cable_ts_path(capture_dir: &Path) -> PathBuf {
    bench_dir(capture_dir).join(CABLE_TS)
}

pub fn terr_ts_path(capture_dir: &Path) -> PathBuf {
    bench_dir(capture_dir).join(TERR_TS)
}

/// Preferred playout file-picker root: bench folder when it exists.
pub fn playout_picker_root(capture_dir: &Path) -> PathBuf {
    let bench = bench_dir(capture_dir);
    if bench.is_dir() {
        bench
    } else {
        capture_dir.to_path_buf()
    }
}

fn cable_tune() -> TuneRecord {
    TuneRecord {
        frequency_mhz: 722,
        polarity: String::new(),
        symbol_rate_ksps: 6876,
        delivery_system: "DVB-C".into(),
        modulation: "256-QAM".into(),
        fec: "auto".into(),
    }
}

fn terr_tune() -> TuneRecord {
    TuneRecord {
        frequency_mhz: 618,
        polarity: String::new(),
        symbol_rate_ksps: 0,
        delivery_system: "DVB-T2".into(),
        modulation: "64-QAM".into(),
        fec: "2/3".into(),
    }
}

/// Writes sidecar JSON next to bench TS files when missing.
pub fn ensure_bench_sidecars(capture_dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(bench_dir(capture_dir))?;
    write_sidecar_if_missing(&cable_ts_path(capture_dir), cable_services_record())?;
    write_sidecar_if_missing(&terr_ts_path(capture_dir), terr_services_record())?;
    Ok(())
}

fn write_sidecar_if_missing(ts: &Path, record: CaptureServicesRecord) -> std::io::Result<()> {
    let sidecar = crate::analyze::services_json_path_for_ts(ts);
    if sidecar.is_file() {
        return Ok(());
    }
    record
        .write_file(&sidecar)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
}

pub fn cable_services_record() -> CaptureServicesRecord {
    CaptureServicesRecord {
        ts_id: None,
        bitrate: None,
        services: Vec::new(),
        captured_at: chrono::Local::now().to_rfc3339(),
        tune: cable_tune(),
    }
}

pub fn terr_services_record() -> CaptureServicesRecord {
    CaptureServicesRecord {
        ts_id: Some(1),
        bitrate: None,
        services: Vec::new(),
        captured_at: chrono::Local::now().to_rfc3339(),
        tune: terr_tune(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bench_paths_under_capture_dir() {
        let root = PathBuf::from("/tmp/captures");
        assert_eq!(
            cable_ts_path(&root),
            PathBuf::from("/tmp/captures/bench/vectra_722_dvbc.ts")
        );
    }
}

//! Session log for the current `dvbhub` process.
//!
//! Each invocation truncates `/tmp/dvbhub_last_log.txt` (expected on tmpfs).
//! Override path with `DVBHUB_LOG=/other/path`; disable with `DVBHUB_LOG=0`.

use chrono::Local;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub const DEFAULT_PATH: &str = "/tmp/dvbhub_last_log.txt";

static LOG: OnceLock<Mutex<LogState>> = OnceLock::new();

struct LogState {
    enabled: bool,
    path: PathBuf,
    file: Option<File>,
}

/// Initializes logging once per process (truncates the log file).
pub fn init() {
    let _ = LOG.get_or_init(|| {
        let enabled = !matches!(
            std::env::var("DVBHUB_LOG").ok().as_deref(),
            Some("0") | Some("false") | Some("off")
        );
        let path = std::env::var("DVBHUB_LOG")
            .ok()
            .filter(|s| !s.is_empty() && s != "0" && s != "1" && s != "true" && s != "false")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_PATH));

        let mut state = LogState {
            enabled,
            path: path.clone(),
            file: None,
        };

        if !enabled {
            return Mutex::new(state);
        }

        let tmpfs = path.parent().map(is_tmpfs_mount).unwrap_or(false);
        state.file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .ok();

        let pid = std::process::id();
        let args: Vec<_> = std::env::args().collect();
        write_raw(
            &mut state,
            &format!(
                "=== dvbhub session pid={pid} tmpfs={tmpfs} log={} ===",
                path.display()
            ),
        );
        write_raw(&mut state, &format!("argv: {}", args.join(" ")));
        if !tmpfs {
            write_raw(
                &mut state,
                "WARN: log path is not on tmpfs — consider DVBHUB_LOG=/tmp/dvbhub_last_log.txt",
            );
        }

        Mutex::new(state)
    });
}

pub fn path() -> PathBuf {
    init();
    LOG.get()
        .and_then(|m| m.lock().ok())
        .map(|s| s.path.clone())
        .unwrap_or_else(|| PathBuf::from(DEFAULT_PATH))
}

pub fn enabled() -> bool {
    init();
    LOG.get()
        .and_then(|m| m.lock().ok())
        .map(|s| s.enabled)
        .unwrap_or(false)
}

/// One timestamped line: `phase` is a short category (`main`, `tsp`, `watch`, …).
pub fn event(phase: &str, detail: &str) {
    init();
    write_event(phase, detail, None);
}

/// Like [`event`] but prefixes detail with `+{elapsed_ms}ms` since `since`.
pub fn event_elapsed(phase: &str, since: std::time::Instant, detail: &str) {
    init();
    let ms = since.elapsed().as_millis();
    write_event(phase, detail, Some(ms));
}

fn write_event(phase: &str, detail: &str, elapsed_ms: Option<u128>) {
    let Some(mutex) = LOG.get() else {
        return;
    };
    let Ok(mut state) = mutex.lock() else {
        return;
    };
    if !state.enabled {
        return;
    }
    let ts = Local::now().format("%H:%M:%S%.3f");
    let prefix = match elapsed_ms {
        Some(ms) => format!("{ts} [{phase}] +{ms}ms {detail}"),
        None => format!("{ts} [{phase}] {detail}"),
    };
    write_raw(&mut state, &prefix);
}

fn write_raw(state: &mut LogState, line: &str) {
    if let Some(f) = state.file.as_mut() {
        let _ = writeln!(f, "{line}");
        let _ = f.flush();
    }
}

/// True when `path` sits on a filesystem mounted as tmpfs (checks `/proc/mounts`).
pub fn is_tmpfs_mount(path: &Path) -> bool {
    let Ok(mounts) = std::fs::read_to_string("/proc/mounts") else {
        return false;
    };
    let Ok(target) = std::fs::canonicalize(path) else {
        return false;
    };
    // The filesystem holding `target` is the one of the longest mount point that
    // is a path-component prefix of it (any type, so nested mounts are honoured).
    let mut best: Option<(usize, bool)> = None;
    for line in mounts.lines() {
        let mut parts = line.split_whitespace();
        let (Some(_dev), Some(mount_point), Some(fstype)) =
            (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        if target.starts_with(Path::new(mount_point)) {
            let len = mount_point.len();
            if best.map_or(true, |(l, _)| len >= l) {
                best = Some((len, fstype == "tmpfs"));
            }
        }
    }
    best.is_some_and(|(_, tmpfs)| tmpfs)
}

/// Returns `fuser` output for a DVB frontend path (empty when free).
pub fn frontend_holder(frontend: &Path) -> String {
    let output = std::process::Command::new("fuser").arg(frontend).output();
    match output {
        Ok(o) if !o.stdout.is_empty() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        Ok(o) if !o.stderr.is_empty() => String::from_utf8_lossy(&o.stderr).trim().to_string(),
        _ => String::new(),
    }
}

/// Best-effort snapshot of watch-related processes.
pub fn related_process_snapshot() -> String {
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg("ps -eo pid,cmd 2>/dev/null | rg 'tsp |dvbhub |mpv |ffmpeg ' || true")
        .output();
    match output {
        Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        Err(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tmpfs_detection_matches_proc_mounts() {
        // Whether /tmp is tmpfs depends on the host (not in Docker, not on all
        // distros), so check against the mount table instead of assuming it.
        let mounts = std::fs::read_to_string("/proc/mounts").unwrap_or_default();
        let tmpfs_mount = mounts.lines().find_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            (f.len() > 2 && f[2] == "tmpfs" && Path::new(f[1]).is_dir()).then(|| f[1].to_string())
        });
        if let Some(mp) = tmpfs_mount {
            assert!(
                is_tmpfs_mount(Path::new(&mp)),
                "{mp} is tmpfs in /proc/mounts"
            );
        }
        assert!(!is_tmpfs_mount(Path::new("/proc")));
    }

    #[test]
    fn default_log_path_under_tmp() {
        assert!(DEFAULT_PATH.starts_with("/tmp/"));
    }
}

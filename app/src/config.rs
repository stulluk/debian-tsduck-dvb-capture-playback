//! Runtime configuration: locating the deployed TSDuck installation.
//!
//! TSDuck is built in Docker and deployed to an isolated, self-contained prefix
//! on the host (default `/opt/tsduck`, see `docker/tsduck/deploy.sh`). The binary
//! resolves the tool directory at runtime so the same binary works regardless of
//! where TSDuck was deployed.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Environment variable that overrides the TSDuck installation directory.
pub const ENV_TSDUCK_DIR: &str = "DVBHUB_TSDUCK_DIR";

/// Default isolated prefix where the TSDuck bundle is deployed on the host.
pub const DEFAULT_TSDUCK_DIR: &str = "/opt/tsduck";

/// Resolved locations of the TSDuck tools used by the orchestrator.
#[derive(Debug, Clone)]
pub struct TsduckPaths {
    /// Directory containing the TSDuck tools (tsp, tsdektec, tslsdvb, ...).
    pub dir: PathBuf,
}

impl TsduckPaths {
    /// Resolves the TSDuck directory from `$DVBHUB_TSDUCK_DIR`, then the default
    /// prefix, then any `tsp` found on `$PATH`. Returns an error if none works.
    pub fn resolve() -> anyhow::Result<Self> {
        if let Some(dir) = std::env::var_os(ENV_TSDUCK_DIR) {
            let dir = PathBuf::from(dir);
            if dir.join("tsp").is_file() {
                return Ok(Self { dir });
            }
            anyhow::bail!(
                "{ENV_TSDUCK_DIR} is set to {} but no tsp binary was found there",
                dir.display()
            );
        }

        let default = Path::new(DEFAULT_TSDUCK_DIR);
        if default.join("tsp").is_file() {
            return Ok(Self {
                dir: default.to_path_buf(),
            });
        }

        if let Some(dir) = find_on_path("tsp") {
            return Ok(Self { dir });
        }

        anyhow::bail!(
            "TSDuck not found. Deploy it (docker/tsduck/deploy.sh) or set {ENV_TSDUCK_DIR}."
        )
    }

    /// Absolute path to a TSDuck tool by name (e.g. "tsp", "tsdektec").
    pub fn tool(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

/// Searches `$PATH` for an executable and returns the directory containing it.
fn find_on_path(exe: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        if dir.join(exe).is_file() {
            return Some(dir);
        }
    }
    None
}

/// The user's home directory, resolved in an OS-agnostic way.
///
/// Uses `%USERPROFILE%` on Windows and `$HOME` elsewhere.
pub fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

/// The per-user configuration directory, following platform conventions:
/// `%APPDATA%` (Windows), `~/Library/Application Support` (macOS),
/// `$XDG_CONFIG_HOME` or `~/.config` (Linux/other).
pub fn config_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        return std::env::var_os("APPDATA").map(PathBuf::from);
    }
    #[cfg(target_os = "macos")]
    {
        return home_dir().map(|h| h.join("Library").join("Application Support"));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        return std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
            .or_else(|| home_dir().map(|h| h.join(".config")));
    }
}

/// Default directory for captures and HLS output: `<home>/dvbhub-captures`.
pub fn default_capture_dir() -> PathBuf {
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("dvbhub-captures")
}

/// Path to the persisted application config (`<config_dir>/dvbhub/config.json`).
pub fn config_file_path() -> PathBuf {
    config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("dvbhub")
        .join("config.json")
}

/// Path to the session activity log (`<config_dir>/dvbhub/dvbhub.log`).
pub fn log_file_path() -> PathBuf {
    config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("dvbhub")
        .join("dvbhub.log")
}

/// The per-user dvbhub data directory (`<config_dir>/dvbhub`), where all
/// persisted JSON databases live. Created on demand by the callers.
pub fn data_dir() -> PathBuf {
    config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("dvbhub")
}

/// Path to the persisted satellite database (`<data_dir>/satellites.json`).
pub fn satellites_file_path() -> PathBuf {
    data_dir().join("satellites.json")
}

/// Path to the user's reference transponders (`<data_dir>/reference_transponders.json`).
pub fn reference_transponders_file_path() -> PathBuf {
    data_dir().join("reference_transponders.json")
}

/// Path to the reference playout captures (`<data_dir>/reference_captures.json`).
pub fn reference_captures_file_path() -> PathBuf {
    data_dir().join("reference_captures.json")
}

/// Path to the saved TV channel catalogue (`<data_dir>/channels.json`).
pub fn channels_file_path() -> PathBuf {
    data_dir().join("channels.json")
}

/// User-editable application settings, persisted to `config.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Directory where captures and HLS segments are written.
    pub capture_dir: PathBuf,
    /// Default capture duration per transponder (TUI / satellite capture).
    #[serde(default = "default_capture_seconds")]
    pub capture_seconds: u32,
}

fn default_capture_seconds() -> u32 {
    30
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            capture_dir: default_capture_dir(),
            capture_seconds: default_capture_seconds(),
        }
    }
}

impl AppConfig {
    /// Loads the config from disk, falling back to defaults if missing/invalid.
    pub fn load() -> Self {
        let path = config_file_path();
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(cfg) = serde_json::from_str::<AppConfig>(&text) {
                return cfg;
            }
        }
        AppConfig::default()
    }

    /// Persists the config to disk, creating parent directories as needed.
    pub fn save(&self) -> std::io::Result<()> {
        let path = config_file_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(&path, text)
    }

    /// Resolves a capture output name to an absolute path. Absolute inputs are
    /// used as-is; relative names are placed inside `capture_dir` (any leading
    /// directory components in the name are preserved).
    pub fn resolve_output(&self, name: &str) -> PathBuf {
        let p = Path::new(name);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.capture_dir.join(name)
        }
    }

    /// Directory for live HLS playback segments.
    pub fn hls_dir(&self) -> PathBuf {
        self.capture_dir.join("hls")
    }

    /// Expands a leading `~` in a path string to the user's home directory.
    pub fn expand_path(path: &str) -> PathBuf {
        let trimmed = path.trim();
        if let Some(rest) = trimmed.strip_prefix("~/") {
            return home_dir()
                .map(|h| h.join(rest))
                .unwrap_or_else(|| PathBuf::from(trimmed));
        }
        if trimmed == "~" {
            return home_dir().unwrap_or_else(|| PathBuf::from("."));
        }
        PathBuf::from(trimmed)
    }
}

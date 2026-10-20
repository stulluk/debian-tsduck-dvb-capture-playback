//! Runtime configuration: locating the deployed TSDuck installation.

use crate::lastlog;
use crate::tools::{find_executable_on_path, is_executable};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Environment variable that overrides the TSDuck tool directory.
pub const ENV_TSDUCK_DIR: &str = "DVBHUB_TSDUCK_DIR";

/// Resolved locations of the TSDuck tools used by the orchestrator.
#[derive(Debug, Clone)]
pub struct TsduckPaths {
    /// Directory containing `tsp` and sibling TSDuck tools.
    pub dir: PathBuf,
}

impl TsduckPaths {
    /// Resolves TSDuck from `$PATH` (`tsp` must be executable).
    ///
    /// Optional override: `$DVBHUB_TSDUCK_DIR` must contain an executable `tsp`.
    pub fn resolve() -> anyhow::Result<Self> {
        let result = Self::resolve_inner();
        match &result {
            Ok(p) => lastlog::event("config", &format!("tsduck dir={}", p.dir.display())),
            Err(e) => lastlog::event("config", &format!("tsduck resolve failed: {e:#}")),
        }
        result
    }

    fn resolve_inner() -> anyhow::Result<Self> {
        if let Some(dir) = std::env::var_os(ENV_TSDUCK_DIR) {
            let dir = PathBuf::from(dir);
            let tsp = dir.join("tsp");
            if is_executable(&tsp) {
                return Ok(Self { dir });
            }
            anyhow::bail!(
                "{ENV_TSDUCK_DIR} is set to {} but no executable `tsp` was found there",
                dir.display()
            );
        }

        if let Some(tsp) = find_executable_on_path("tsp") {
            let dir = tsp
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));
            return Ok(Self { dir });
        }

        // Default deploy prefix used by docker/tsduck/deploy.sh (works under sudo
        // even when secure_path omits /opt/tsduck).
        let default_dir = PathBuf::from("/opt/tsduck");
        if is_executable(&default_dir.join("tsp")) {
            return Ok(Self { dir: default_dir });
        }

        anyhow::bail!("{}", tsp_not_found_message())
    }

    /// Absolute path to a TSDuck tool by name (e.g. "tsp", "tsdektec").
    pub fn tool(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

fn tsp_not_found_message() -> String {
    format!(
        "TSDuck `tsp` not found on $PATH.\n\
         \n\
         dvbhub requires a TSDuck build with Linux DVB and (for DTU-315 playout) Dektec support.\n\
         \n\
         Build and install from this repository:\n\
           ./docker/tsduck/dockerbuild.sh\n\
           ./docker/tsduck/indockerbuild.sh\n\
           ./docker/tsduck/deploy.sh          # installs under /opt/tsduck — add to PATH\n\
         \n\
         See docs/tsduck_build.md for DTAPI / LinuxSDK details (Dektec DTU-315 needs DTAPI).\n\
         \n\
         After install, ensure `tsp` is on your PATH, or set:\n\
           export {ENV_TSDUCK_DIR}=/path/to/tsduck-bundle\n\
         \n\
         Related project: https://gitlab.com/stulluk/tsduck-capture-playback"
    )
}

/// The user's home directory, resolved in an OS-agnostic way.
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

/// The per-user configuration directory, following platform conventions.
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

/// The per-user dvbhub data directory (`<config_dir>/dvbhub`).
pub fn data_dir() -> PathBuf {
    config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("dvbhub")
}

pub fn satellites_file_path() -> PathBuf {
    data_dir().join("satellites.json")
}

pub fn reference_transponders_file_path() -> PathBuf {
    data_dir().join("reference_transponders.json")
}

pub fn reference_captures_file_path() -> PathBuf {
    data_dir().join("reference_captures.json")
}

pub fn channels_file_path() -> PathBuf {
    data_dir().join("channels.json")
}

/// User-editable application settings, persisted to `config.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub capture_dir: PathBuf,
    #[serde(default = "default_capture_seconds")]
    pub capture_seconds: u32,
    /// Browser playback quality: "720p" or "source" (native resolution).
    #[serde(default = "default_web_quality")]
    pub web_quality: String,
}

fn default_capture_seconds() -> u32 {
    30
}

fn default_web_quality() -> String {
    "720p".into()
}

/// Accepted values for [`AppConfig::web_quality`].
pub const WEB_QUALITIES: &[&str] = &["720p", "source"];

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            capture_dir: default_capture_dir(),
            capture_seconds: default_capture_seconds(),
            web_quality: default_web_quality(),
        }
    }
}

impl AppConfig {
    pub fn load() -> Self {
        let path = config_file_path();
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(cfg) = serde_json::from_str::<AppConfig>(&text) {
                return cfg;
            }
        }
        AppConfig::default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = config_file_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(&path, text)
    }

    pub fn resolve_output(&self, name: &str) -> PathBuf {
        let p = Path::new(name);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.capture_dir.join(name)
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tsp_not_found_message_mentions_build_docs() {
        let msg = tsp_not_found_message();
        assert!(msg.contains("docs/tsduck_build.md"));
        assert!(msg.contains("tsp"));
    }
}

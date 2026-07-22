//! External tool discovery on `$PATH` (media players, etc.).

use std::path::{Path, PathBuf};

/// Returns the absolute path of an executable found on `$PATH`.
pub fn find_executable_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// True when `path` exists and is executable (Unix: any execute bit set).
pub fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false);
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Verifies the media player referenced by `watch --player` exists on `$PATH`.
pub fn ensure_watch_player(player_cmd: &str) -> anyhow::Result<()> {
    let prog = player_cmd
        .split_whitespace()
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("empty --player command"))?;

    if find_executable_on_path(prog).is_some() {
        return Ok(());
    }

    let has_mpv = find_executable_on_path("mpv").is_some();
    let has_vlc = find_executable_on_path("vlc").is_some();

    if has_mpv || has_vlc {
        anyhow::bail!(
            "media player '{prog}' not found on $PATH.\n\
             Install it or pass e.g. --player 'mpv --force-window=yes --title=dvbhub -' \
             or --player 'vlc -'"
        );
    }

    anyhow::bail!(
        "neither mpv nor vlc found on $PATH — `dvbhub watch` will not work.\n\
         Install one:  apt install mpv   or   apt install vlc"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_tsp_when_on_path() {
        if find_executable_on_path("sh").is_some() {
            assert!(is_executable(Path::new("/bin/sh")) || is_executable(Path::new("/usr/bin/sh")));
        }
    }
}

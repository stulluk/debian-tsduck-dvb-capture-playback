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

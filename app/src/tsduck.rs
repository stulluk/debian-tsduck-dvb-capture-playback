//! Thin wrapper around the TSDuck command-line tools.
//!
//! The orchestrator drives DVB operations by spawning TSDuck tools as
//! subprocesses and parsing their output (JSON where available). This module
//! centralizes process invocation and error reporting.

use crate::config::TsduckPaths;
use anyhow::Context;
use std::process::Command;

use crate::lastlog;

/// Runs a TSDuck tool with the given args, inheriting stdio (for live output),
/// and returns whether it exited successfully.
///
/// The child gets SIGTERM when dvbhub dies for any reason (SIGTERM, SIGINT, even
/// SIGKILL), so a `tsp` never outlives dvbhub and keeps holding the DTU-315
/// (which made the next playout fail with `DTAPI_E_IN_USE`).
pub fn run_inherit(paths: &TsduckPaths, tool: &str, args: &[String]) -> anyhow::Result<bool> {
    use std::os::unix::process::CommandExt;
    lastlog::event("tsp", &format!("inherit {}", display_command(tool, args)));
    let bin = paths.tool(tool);
    let mut cmd = Command::new(&bin);
    cmd.args(args);
    // SAFETY: prctl is async-signal-safe and only affects the forked child.
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    let status = cmd
        .status()
        .with_context(|| format!("failed to launch {}", bin.display()))?;
    let ok = status.success();
    lastlog::event(
        "tsp",
        &format!("inherit exit ok={ok} code={}", status.code().unwrap_or(-1)),
    );
    Ok(ok)
}

/// Runs a TSDuck tool and captures its stdout as a UTF-8 string.
///
/// Returns an error if the tool cannot be launched or exits with a non-zero
/// status. stderr is included in the error context for diagnostics.
pub fn run_capture(paths: &TsduckPaths, tool: &str, args: &[&str]) -> anyhow::Result<String> {
    lastlog::event("tsp", &format!("capture {tool} {}", args.join(" ")));
    let bin = paths.tool(tool);
    let output = Command::new(&bin)
        .args(args)
        .output()
        .with_context(|| format!("failed to launch {}", bin.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        lastlog::event("tsp", &format!("capture failed: {}", stderr.trim()));
        anyhow::bail!(
            "{} {} exited with {}: {}",
            bin.display(),
            args.join(" "),
            output.status,
            stderr.trim()
        );
    }

    String::from_utf8(output.stdout)
        .with_context(|| format!("{} produced non-UTF-8 output", bin.display()))
}

/// Runs a TSDuck tool and parses its stdout as JSON.
pub fn run_json(
    paths: &TsduckPaths,
    tool: &str,
    args: &[&str],
) -> anyhow::Result<serde_json::Value> {
    let stdout = run_capture(paths, tool, args)?;
    serde_json::from_str(&stdout)
        .with_context(|| format!("failed to parse JSON output of {tool}"))
}

/// Builds a `Command` for a TSDuck tool, ready for the caller to add args.
pub fn tool_command(paths: &TsduckPaths, tool: &str) -> Command {
    Command::new(paths.tool(tool))
}

/// Returns the TSDuck version string (e.g. "TSDuck ... version 3.44-4676").
///
/// `tsp --version` writes to stderr, so this reads both streams and returns the
/// first non-empty line, stripped of the leading "tsp: " prefix.
pub fn version(paths: &TsduckPaths) -> Option<String> {
    let output = Command::new(paths.tool("tsp"))
        .arg("--version")
        .output()
        .ok()?;
    let text = if output.stdout.is_empty() {
        String::from_utf8_lossy(&output.stderr).into_owned()
    } else {
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    text.lines()
        .map(|l| l.trim().trim_start_matches("tsp:").trim())
        .find(|l| !l.is_empty())
        .map(|l| l.to_string())
}

/// Formats a tool + args as a copy-pasteable shell command line.
pub fn display_command(tool: &str, args: &[String]) -> String {
    let mut line = tool.to_string();
    for a in args {
        line.push(' ');
        if a.is_empty() || a.contains(char::is_whitespace) {
            line.push('"');
            line.push_str(a);
            line.push('"');
        } else {
            line.push_str(a);
        }
    }
    line
}

/// Plain-language hint for well-known TSDuck / DTAPI / tuner error lines.
pub fn explain_error(line: &str) -> Option<&'static str> {
    if line.contains("DTAPI_E_EVENT_POWER") {
        Some(
            "the DTU-315 lost USB power while modulating — connect it directly to a host \
             USB port or a powered hub (bus-powered hubs cannot supply it)",
        )
    } else if line.contains("no input signal lock") {
        Some("no signal: check the cable, the tuning parameters, LNB / DiSEqC settings")
    } else if line.contains("Device or resource busy") {
        Some("the tuner is in use by another program (another dvbhub, tsp, kaffeine, …)")
    } else {
        None
    }
}

#[cfg(test)]
mod explain_tests {
    use super::explain_error;

    #[test]
    fn explains_dtu_power_loss() {
        let hint = explain_error("* Error: dektec: error muting modulator output: DTAPI_E_EVENT_POWER (DTAPI status 4230)");
        assert!(hint.unwrap().contains("USB power"));
        assert!(explain_error("all good").is_none());
    }
}

//! Thin wrapper around the TSDuck command-line tools.
//!
//! The orchestrator drives DVB operations by spawning TSDuck tools as
//! subprocesses and parsing their output (JSON where available). This module
//! centralizes process invocation and error reporting.

use crate::config::TsduckPaths;
use anyhow::Context;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;

use crate::lastlog;

/// Runs a TSDuck tool with the given args, inheriting stdio (for live output),
/// and returns whether it exited successfully.
pub fn run_inherit(paths: &TsduckPaths, tool: &str, args: &[String]) -> anyhow::Result<bool> {
    lastlog::event("tsp", &format!("inherit {}", display_command(tool, args)));
    let bin = paths.tool(tool);
    let status = Command::new(&bin)
        .args(args)
        .status()
        .with_context(|| format!("failed to launch {}", bin.display()))?;
    let ok = status.success();
    lastlog::event(
        "tsp",
        &format!("inherit exit ok={ok} code={}", status.code().unwrap_or(-1)),
    );
    Ok(ok)
}

/// Like [`run_inherit`] but captures stderr into `on_stderr` while stdout stays inherited.
pub fn run_inherit_log_stderr<F>(
    paths: &TsduckPaths,
    tool: &str,
    args: &[String],
    mut on_stderr: F,
) -> anyhow::Result<(bool, i32)>
where
    F: FnMut(&str) + Send + 'static,
{
    let bin = paths.tool(tool);
    let mut child = Command::new(&bin)
        .args(args)
        .stdout(Stdio::inherit())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to launch {}", bin.display()))?;

    let pid = child.id();
    if let Some(stderr) = child.stderr.take() {
        let mut cb = on_stderr;
        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                cb(&line);
            }
        });
    }

    let status = child.wait().with_context(|| format!("wait failed pid={pid}"))?;
    Ok((status.success(), status.code().unwrap_or(-1)))
}

/// Runs with `--verbose` on stderr logged; mirrors lines to terminal when `mirror` is true.
pub fn run_inherit_verbose(
    paths: &TsduckPaths,
    tool: &str,
    args: &[String],
    mirror_stderr: bool,
    on_stderr: Arc<dyn Fn(&str) + Send + Sync>,
) -> anyhow::Result<(bool, i32)> {
    lastlog::event(
        "tsp",
        &format!("verbose {}", display_command(tool, args)),
    );
    let bin = paths.tool(tool);
    let mut argv = vec!["--verbose".to_string()];
    argv.extend(args.iter().cloned());
    let mut child = Command::new(&bin)
        .args(&argv)
        .stdout(Stdio::inherit())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to launch {}", bin.display()))?;

    let pid = child.id();
    if let Some(stderr) = child.stderr.take() {
        let log = Arc::clone(&on_stderr);
        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                log(&line);
                if mirror_stderr {
                    eprintln!("tsp: {line}");
                }
            }
        });
    }

    let status = child
        .wait()
        .with_context(|| format!("wait failed pid={pid}"))?;
    let code = status.code().unwrap_or(-1);
    lastlog::event("tsp", &format!("verbose exit ok={} code={code}", status.success()));
    Ok((status.success(), code))
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

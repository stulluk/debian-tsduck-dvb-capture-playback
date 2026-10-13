//! Plain-text CLI progress for long-running `tsp` operations.
//!
//! Prints staged status lines (`[dvbhub] …`) while a capture runs. The same
//! event hooks drive the ratatui capture view (`quiet` mode).

use crate::analyze::{
    feed_json_chunk, print_live_services, services_display_lines, summary_from_analyze_json,
    StreamSummary,
};
use crate::config::TsduckPaths;
use crate::fe_stats::{self, FeMonitor, SignalStats};
use crate::pipeline::CaptureParams;
use crate::signal::{parse_dvbstatus_line, parse_signal_line, SignalStatus};
use crate::tsduck;
use crate::lastlog;
use anyhow::Context;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Live capture events for the ratatui run view (and other UIs).
#[derive(Debug, Clone)]
pub enum CaptureEvent {
    /// Full `tsp` command line (`tsp --verbose …`).
    Command(String),
    /// High-level phase (`Starting tsp`, `Waiting for lock`, `Recording 5 s`).
    Status(String),
    /// stderr line worth showing in the log pane.
    Log(String),
    /// RF lock / quality update from tsp verbose output.
    Signal(SignalStatus),
    /// Service names discovered by live analyze.
    Services(Vec<String>),
    /// Seconds left while recording after lock.
    RecordingRemaining(u32),
}

/// Prints the capture plan before `tsp` is started.
pub fn print_capture_intro(
    paths: &TsduckPaths,
    params: &CaptureParams,
    transponder_hint: Option<&str>,
    device_label: Option<&str>,
) {
    note(&format!(
        "TSDuck {} @ {}",
        tsduck::version(paths).unwrap_or_else(|| "unknown".into()),
        paths.dir.display()
    ));

    let adapter_guess = params.adapter.map(|a| format!("/dev/dvb/adapter{a}"));
    let dev = params
        .device_name
        .as_deref()
        .or(adapter_guess.as_deref())
        .unwrap_or("default");
    if let Some(label) = device_label {
        note(&format!("Device: {dev} ({label})"));
    } else {
        note(&format!("Device: {dev}"));
    }

    let mhz = params.frequency_hz / 1_000_000;
    let pol = params.polarity.as_deref().unwrap_or("—");
    let sr = params
        .symbol_rate
        .map(|s| format!("{}", s / 1_000))
        .unwrap_or_else(|| "—".into());
    let modu = params.modulation.as_deref().unwrap_or("—");
    let fec = params.fec.as_deref().unwrap_or("—");
    let diseqc = params
        .satellite_number
        .map(|n| format!("DiSEqC port {n}"))
        .unwrap_or_else(|| "DiSEqC default".into());

    if let Some(hint) = transponder_hint {
        note(&format!("Transponder: {hint}"));
    }
    note(&format!(
        "Tune: {mhz} MHz {pol}, {}, SR {sr} kSym/s, {modu} FEC {fec}, {diseqc}",
        params.delivery_system
    ));
    note(&format!("Signal timeout: {} s", params.signal_timeout_s));

    if params.seconds > 0 {
        note(&format!(
            "Output: {} ({} s capture)",
            params.output, params.seconds
        ));
    } else {
        note(&format!("Output: {} (until interrupted)", params.output));
    }

    if params.live_analyze {
        note("Live analyze enabled — service list appears after lock (PAT/SDT)");
    }
}

/// Outcome of a capture with progress reporting.
#[derive(Debug, Clone)]
pub struct CaptureRun {
    pub success: bool,
    pub ever_locked: bool,
    pub services_found: bool,
    pub killed_by_watchdog: bool,
    pub last_signal: Option<SignalStatus>,
    pub last_summary: Option<StreamSummary>,
    /// C/N / level / post-BER statistics sampled with `dvb-fe-tool` while locked.
    pub signal_stats: Option<SignalStats>,
}

/// Upper bound for one bounded capture: lock wait + recording + teardown slack.
pub fn capture_watchdog_secs(signal_timeout_s: u32, record_seconds: u32) -> Option<u64> {
    if record_seconds == 0 {
        None
    } else {
        Some(
            signal_timeout_s as u64
                + record_seconds as u64
                + 30,
        )
    }
}

/// Runs `tsp --verbose` with `[dvbhub]` progress on stderr.
pub fn run_capture_progress(
    paths: &TsduckPaths,
    args: &[String],
    seconds: u32,
    signal_timeout_s: u32,
) -> anyhow::Result<CaptureRun> {
    run_capture_inner(paths, args, seconds, signal_timeout_s, false, None, None)
}

/// Same as [`run_capture_progress`] but suppresses stderr progress (for TUI).
pub fn run_capture_quiet(
    paths: &TsduckPaths,
    args: &[String],
    seconds: u32,
    signal_timeout_s: u32,
    cancel: Option<Arc<AtomicBool>>,
) -> anyhow::Result<CaptureRun> {
    run_capture_inner(paths, args, seconds, signal_timeout_s, true, None, cancel)
}

/// Runs capture and pushes [`CaptureEvent`] updates to `events` (TUI log pane).
pub fn run_capture_with_events(
    paths: &TsduckPaths,
    args: &[String],
    seconds: u32,
    signal_timeout_s: u32,
    events: Sender<CaptureEvent>,
    cancel: Option<Arc<AtomicBool>>,
) -> anyhow::Result<CaptureRun> {
    run_capture_inner(
        paths,
        args,
        seconds,
        signal_timeout_s,
        true,
        Some(events),
        cancel,
    )
}

fn emit(
    events: &Option<Sender<CaptureEvent>>,
    quiet: bool,
    event: CaptureEvent,
) {
    if let Some(tx) = events {
        let _ = tx.send(event);
        return;
    }
    if quiet {
        return;
    }
    match event {
        CaptureEvent::Status(msg) | CaptureEvent::Log(msg) => note(&msg),
        CaptureEvent::Command(cmd) => note(&format!("Command: {cmd}")),
        CaptureEvent::Signal(sig) => note(&sig.summary()),
        CaptureEvent::Services(lines) => {
            if lines.is_empty() {
                return;
            }
            if lines.len() == 1 {
                note(&lines[0]);
                return;
            }
            for line in lines {
                note(&line);
            }
        }
        CaptureEvent::RecordingRemaining(left) => {
            note(&format!("Recording… {left} s remaining"));
        }
    }
}

fn wait_child_with_watchdog(
    child: &mut Child,
    watchdog_secs: Option<u64>,
    cancel: Option<&AtomicBool>,
    events: &Option<Sender<CaptureEvent>>,
    quiet: bool,
) -> anyhow::Result<(std::process::ExitStatus, bool)> {
    let poll_cancel = |child: &mut Child| -> anyhow::Result<Option<std::process::ExitStatus>> {
        if cancel.is_some_and(|c| c.load(Ordering::SeqCst)) {
            emit(
                events,
                quiet,
                CaptureEvent::Log("Capture cancelled — stopping tsp".into()),
            );
            let _ = child.kill();
            let status = child.wait().context("waiting for tsp after cancel")?;
            return Ok(Some(status));
        }
        Ok(None)
    };

    let Some(limit) = watchdog_secs else {
        loop {
            if let Some(status) = poll_cancel(child)? {
                return Ok((status, false));
            }
            match child.try_wait() {
                Ok(Some(status)) => return Ok((status, false)),
                Ok(None) => thread::sleep(Duration::from_millis(200)),
                Err(e) => return Err(e.into()),
            }
        }
    };
    let deadline = Instant::now() + Duration::from_secs(limit);
    loop {
        if let Some(status) = poll_cancel(child)? {
            return Ok((status, false));
        }
        match child.try_wait() {
            Ok(Some(status)) => return Ok((status, false)),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let msg = format!(
                        "watchdog: tsp exceeded {limit}s (signal-timeout + capture + margin) — killing"
                    );
                    emit(events, quiet, CaptureEvent::Log(msg.clone()));
                    if !quiet && events.is_none() {
                        note(&msg);
                    }
                    let _ = child.kill();
                    let status = child.wait().context("waiting for tsp after kill")?;
                    return Ok((status, true));
                }
                thread::sleep(Duration::from_millis(200));
            }
            Err(e) => return Err(e.into()),
        }
    }
}

fn run_capture_inner(
    paths: &TsduckPaths,
    args: &[String],
    seconds: u32,
    signal_timeout_s: u32,
    quiet: bool,
    events: Option<Sender<CaptureEvent>>,
    cancel: Option<Arc<AtomicBool>>,
) -> anyhow::Result<CaptureRun> {
    let bin = paths.tool("tsp");
    let mut cmd = Command::new(&bin);
    cmd.arg("--verbose").args(args);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    emit(
        &events,
        quiet,
        CaptureEvent::Command(tsduck::display_command("tsp", args)),
    );
    lastlog::event("capture", &tsduck::display_command("tsp", args));
    emit(&events, quiet, CaptureEvent::Status("Starting tsp…".into()));

    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to launch {}", bin.display()))?;

    let stderr = child.stderr.take().expect("stderr piped");
    let stdout = child.stdout.take();
    let ever_locked = Arc::new(AtomicBool::new(false));
    let child_done = Arc::new(AtomicBool::new(false));
    let services_found = Arc::new(AtomicBool::new(false));
    let recording_announced = Arc::new(AtomicBool::new(false));
    let last_signal: Arc<Mutex<Option<SignalStatus>>> = Arc::new(Mutex::new(None));
    let last_summary: Arc<Mutex<Option<StreamSummary>>> = Arc::new(Mutex::new(None));

    // Frontend statistics sampler, started on first lock (dvb input only).
    let fe_target = fe_stats::adapter_frontend_from_args(args);
    let fe_monitor: Arc<Mutex<Option<FeMonitor>>> = Arc::new(Mutex::new(None));
    if fe_target.is_some() && !FeMonitor::available() {
        emit(
            &events,
            quiet,
            CaptureEvent::Log(format!(
                "{} not found — no C/N / BER statistics (apt install dvb-tools)",
                fe_stats::FE_TOOL
            )),
        );
    }

    let el = ever_locked.clone();
    let ls = last_signal.clone();
    let fe_mon = fe_monitor.clone();
    let quiet_stderr = quiet;
    let ev_stderr = events.clone();
    let stderr_thread = thread::spawn(move || {
        let reader = BufReader::new(stderr);
        let mut lock_announced = false;
        for line in reader.lines().map_while(Result::ok) {
            if let Some(sig) = parse_signal_line(&line).or_else(|| parse_dvbstatus_line(&line)) {
                if sig.locked {
                    el.store(true, Ordering::SeqCst);
                    if !lock_announced {
                        lock_announced = true;
                        if let Some((adapter, frontend)) = fe_target {
                            if FeMonitor::available() {
                                match FeMonitor::start(adapter, frontend) {
                                    Ok(mon) => {
                                        if let Ok(mut guard) = fe_mon.lock() {
                                            *guard = Some(mon);
                                        }
                                    }
                                    Err(e) => emit(
                                        &ev_stderr,
                                        quiet_stderr,
                                        CaptureEvent::Log(format!(
                                            "cannot start {}: {e}",
                                            fe_stats::FE_TOOL
                                        )),
                                    ),
                                }
                            }
                        }
                    }
                }
                if let Ok(mut guard) = ls.lock() {
                    *guard = Some(sig.clone());
                }
                emit(
                    &ev_stderr,
                    quiet_stderr,
                    CaptureEvent::Signal(sig),
                );
            } else if line.contains("error") || line.contains("Error") {
                lastlog::event("tsp", line.trim());
                emit(
                    &ev_stderr,
                    quiet_stderr,
                    CaptureEvent::Log(format!("tsp: {}", line.trim())),
                );
            } else if line.starts_with('*') || line.contains("dvb:") {
                lastlog::event("tsp", line.trim());
                emit(
                    &ev_stderr,
                    quiet_stderr,
                    CaptureEvent::Log(line.trim().to_string()),
                );
            }
        }
    });

    let sf = services_found.clone();
    let sum_store = last_summary.clone();
    let quiet_stdout = quiet;
    let ev_stdout = events.clone();
    let stdout_thread = stdout.map(|stdout| {
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            let mut json_buf = String::new();
            let mut json_depth = 0;
            for line in reader.lines().map_while(Result::ok) {
                let Some(doc) = feed_json_chunk(&mut json_buf, &mut json_depth, &line) else {
                    continue;
                };
                let Some(summary) = summary_from_analyze_json(&doc) else {
                    continue;
                };
                if let Ok(mut guard) = sum_store.lock() {
                    *guard = Some(summary.clone());
                }
                if !summary.services.is_empty() {
                    let announce = !sf.load(Ordering::SeqCst);
                    sf.store(true, Ordering::SeqCst);
                    if announce {
                        if ev_stdout.is_some() {
                            let lines = services_display_lines(&summary);
                            emit(
                                &ev_stdout,
                                quiet_stdout,
                                CaptureEvent::Services(lines),
                            );
                        } else if !quiet_stdout {
                            print_live_services(&summary);
                        }
                    }
                }
            }
        })
    });

    emit(
        &events,
        quiet,
        CaptureEvent::Status("Waiting for signal lock…".into()),
    );

    let ticker = if seconds > 0 {
        let ra = recording_announced.clone();
        let el = ever_locked.clone();
        let done = child_done.clone();
        let ev_ticker = events.clone();
        Some(thread::spawn(move || {
            loop {
                thread::sleep(Duration::from_secs(1));
                if el.load(Ordering::SeqCst) || done.load(Ordering::SeqCst) {
                    if el.load(Ordering::SeqCst)
                        && ra
                            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                            .is_ok()
                    {
                        emit(
                            &ev_ticker,
                            quiet,
                            CaptureEvent::Status(format!("Recording {seconds} s…")),
                        );
                    }
                    break;
                }
            }
            if !el.load(Ordering::SeqCst) {
                return;
            }
            let mut elapsed = 0u32;
            while elapsed < seconds && !done.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_secs(1));
                elapsed = elapsed.saturating_add(1);
                if done.load(Ordering::SeqCst) {
                    break;
                }
                let left = seconds.saturating_sub(elapsed);
                if left > 0 {
                    emit(
                        &ev_ticker,
                        quiet,
                        CaptureEvent::RecordingRemaining(left),
                    );
                }
            }
        }))
    } else {
        let ra = recording_announced.clone();
        let el = ever_locked.clone();
        let done = child_done.clone();
        let ev_ticker = events.clone();
        Some(thread::spawn(move || {
            loop {
                thread::sleep(Duration::from_secs(1));
                if el.load(Ordering::SeqCst) || done.load(Ordering::SeqCst) {
                    if el.load(Ordering::SeqCst)
                        && ra
                            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                            .is_ok()
                    {
                        emit(
                            &ev_ticker,
                            quiet,
                            CaptureEvent::Status("Recording (Ctrl-C to stop)…".into()),
                        );
                    }
                    break;
                }
            }
        }))
    };

    let watchdog_secs = capture_watchdog_secs(signal_timeout_s, seconds);
    let cancel_ref = cancel.as_deref();
    let (status, killed_by_watchdog) =
        wait_child_with_watchdog(&mut child, watchdog_secs, cancel_ref, &events, quiet)?;
    child_done.store(true, Ordering::SeqCst);
    let _ = stderr_thread.join();
    if let Some(t) = stdout_thread {
        let _ = t.join();
    }
    if let Some(t) = ticker {
        let _ = t.join();
    }

    let signal_stats = fe_monitor
        .lock()
        .ok()
        .and_then(|mut g| g.take())
        .and_then(FeMonitor::stop);
    if let Some(stats) = &signal_stats {
        lastlog::event("capture", &stats.summary());
        emit(&events, quiet, CaptureEvent::Log(stats.summary()));
    }

    let locked = ever_locked.load(Ordering::SeqCst);
    let last = last_signal.lock().ok().and_then(|g| g.clone());
    let summary = last_summary.lock().ok().and_then(|g| g.clone());

    lastlog::event(
        "capture",
        &format!(
            "done success={} locked={} services={} watchdog={}",
            status.success() && !killed_by_watchdog,
            locked,
            services_found.load(Ordering::SeqCst),
            killed_by_watchdog
        ),
    );

    Ok(CaptureRun {
        success: status.success() && !killed_by_watchdog,
        ever_locked: locked,
        services_found: services_found.load(Ordering::SeqCst) || summary.as_ref().is_some_and(|s| !s.services.is_empty()),
        killed_by_watchdog,
        last_signal: last,
        last_summary: summary,
        signal_stats,
    })
}

/// Prints capture completion summary (file size, lock state).
pub fn print_capture_done(output: &str, run: &CaptureRun) {
    if run.killed_by_watchdog {
        note("ERROR: tsp killed by watchdog (hung past signal-timeout + capture window)");
    } else if run.success {
        match std::fs::metadata(output) {
            Ok(meta) => note(&format!(
                "Done — saved {} ({})",
                output,
                format_bytes(meta.len())
            )),
            Err(_) => note(&format!("Done — saved {output}")),
        }
    } else {
        note("ERROR: tsp exited with an error");
    }

    if !run.ever_locked {
        note("Warning: signal was never locked — check dish, LNB power, and DiSEqC port");
    } else if let Some(sig) = &run.last_signal {
        if !sig.locked {
            note(&format!("Last signal state: {}", sig.summary()));
        }
    }

    if run.ever_locked && !run.services_found {
        note("No service names from live analyze (SDT not yet seen or empty PAT)");
    }
}

fn note(msg: &str) {
    eprintln!("[dvbhub] {msg}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watchdog_secs_bounded_capture() {
        assert_eq!(capture_watchdog_secs(10, 10), Some(50));
        assert_eq!(capture_watchdog_secs(10, 30), Some(70));
    }

    #[test]
    fn watchdog_disabled_for_open_ended() {
        assert_eq!(capture_watchdog_secs(10, 0), None);
    }
}

fn format_bytes(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1} MB", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1} KB", n as f64 / 1_000.0)
    } else {
        format!("{n} B")
    }
}

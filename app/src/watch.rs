//! Live watch: `tsp -I dvb … -P zap <sid> -O file | <player> -`.
//!
//! One deliberately simple pipeline. `tsp` tunes and zaps to one service and
//! writes the single-program TS to stdout; the player (mpv, else vlc, or a
//! user `--player` command) reads it from stdin and does demux, decode and
//! A/V sync itself. dvbhub only manages the two processes:
//!
//! * the player exits (window closed) → `tsp` is stopped (SIGTERM);
//! * `tsp` exits (no lock, device busy) → the player is stopped and the last
//!   `tsp` error is reported.
//!
//! `--test` replaces the player with `ffmpeg -f null` and reports how many
//! video frames were decoded in real time, so playback can be verified
//! headless.

use crate::channels::{ChannelDelivery, SavedChannel};
use crate::config::TsduckPaths;
use crate::devices::{DeliveryCategory, Discovery};
use crate::lastlog;
use crate::tools::find_executable_on_path;
use crate::tsduck;
use anyhow::Context;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub const WATCH_SIGNAL_TIMEOUT_S: u32 = 10;

/// How long `tsp` gets to exit after SIGTERM before it is killed.
const TSP_STOP_GRACE: Duration = Duration::from_secs(3);

/// mpv's own log of the last watch session.
pub const MPV_LOG: &str = "/tmp/dvbhub_mpv_log.txt";

/// Set by the SIGINT/SIGTERM handler while a watch session runs.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_interrupt(_sig: libc::c_int) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

/// Routes SIGINT/SIGTERM to [`INTERRUPTED`] so both children get stopped cleanly.
fn install_interrupt_handler() {
    INTERRUPTED.store(false, Ordering::SeqCst);
    let handler = on_interrupt as extern "C" fn(libc::c_int) as libc::sighandler_t;
    unsafe {
        libc::signal(libc::SIGINT, handler);
        libc::signal(libc::SIGTERM, handler);
    }
}

/// Makes the child receive SIGTERM if dvbhub dies for any reason (even SIGKILL),
/// so a tuner is never left held by an orphaned `tsp`.
fn die_with_parent(cmd: &mut Command) -> &mut Command {
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        })
    }
}

/// Options for one watch session.
#[derive(Debug, Clone, Default)]
pub struct WatchOpts {
    /// Player command line reading TS from stdin; `None` = mpv, else vlc.
    pub player_cmd: Option<String>,
    /// Do not open an audio output (works around broken audio stacks).
    pub no_audio: bool,
    /// Headless verification: decode with ffmpeg for this many seconds.
    pub test_secs: Option<u32>,
    /// Window title (channel name).
    pub title: Option<String>,
}

/// Result of a headless `--test` run (ffmpeg `-progress` output).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TestReport {
    pub frames: u64,
    pub fps: f64,
    pub media_secs: f64,
    pub speed: f64,
    pub decode_errors: usize,
}

impl TestReport {
    /// Pass when frames flowed for most of the window at about real-time speed.
    pub fn passed(&self, secs: u32) -> bool {
        self.frames > 0 && self.media_secs >= f64::from(secs) * 0.8 && self.speed >= 0.8
    }

    /// One-line summary.
    pub fn summary(&self) -> String {
        format!(
            "{} video frames decoded, {:.1} s of media, {:.1} fps, speed {:.2}x, {} decode errors",
            self.frames, self.media_secs, self.fps, self.speed, self.decode_errors
        )
    }
}

/// Outcome of a watch session.
#[derive(Debug, Clone, PartialEq)]
pub struct WatchEnd {
    /// The player closed while `tsp` was still streaming (normal user exit).
    pub user_closed: bool,
    /// `tsp` stopped on its own; carries its last error line if any.
    pub tsp_error: Option<String>,
    /// Filled in `--test` mode.
    pub test: Option<TestReport>,
}

/// Builds the player argv. Order: explicit command, mpv, vlc.
pub fn player_argv(opts: &WatchOpts) -> anyhow::Result<Vec<String>> {
    if let Some(secs) = opts.test_secs {
        anyhow::ensure!(
            find_executable_on_path("ffmpeg").is_some(),
            "watch --test needs ffmpeg (apt install ffmpeg)"
        );
        return Ok(ffmpeg_test_argv(secs));
    }
    if let Some(cmd) = opts.player_cmd.as_deref().filter(|c| !c.trim().is_empty()) {
        let argv: Vec<String> = cmd.split_whitespace().map(str::to_string).collect();
        anyhow::ensure!(
            find_executable_on_path(&argv[0]).is_some(),
            "player '{}' not found on $PATH",
            argv[0]
        );
        return Ok(argv);
    }
    let title = opts.title.clone().unwrap_or_else(|| "dvbhub".into());
    if find_executable_on_path("mpv").is_some() {
        let mut argv = vec![
            "mpv".to_string(),
            "--force-window=yes".into(),
            format!("--title=dvbhub — {title}"),
            "--cache=yes".into(),
            "--demuxer-readahead-secs=0".into(),
            // Default probing (5 MB) waits ~8 s on channels whose AC-3 tracks
            // never yield parameters; 2 MB / 1 s opens video in ~1.9 s and
            // still detects 1080p H.264 reliably (1 MB misdetected the rate).
            "--demuxer-lavf-probesize=2000000".into(),
            "--demuxer-lavf-analyzeduration=1".into(),
            // mpv is silent when stdout is not a terminal; keep its own log
            // (tmpfs) so "VO:/AO:" and dropped frames can be checked later.
            format!("--log-file={MPV_LOG}"),
        ];
        if opts.no_audio {
            argv.push("--no-audio".into());
        }
        argv.push("-".into());
        return Ok(argv);
    }
    if find_executable_on_path("vlc").is_some() {
        let mut argv = vec![
            "vlc".to_string(),
            "--play-and-exit".into(),
            format!("--meta-title=dvbhub — {title}"),
        ];
        if opts.no_audio {
            argv.push("--no-audio".into());
        }
        argv.push("-".into());
        return Ok(argv);
    }
    anyhow::bail!("neither mpv nor vlc found on $PATH — install one: apt install mpv")
}

/// `ffmpeg` argv that decodes the first video stream for `secs` and prints
/// machine-readable progress on stdout.
fn ffmpeg_test_argv(secs: u32) -> Vec<String> {
    [
        "ffmpeg",
        "-hide_banner",
        "-nostats",
        "-loglevel",
        "error",
        "-i",
        "-",
        "-map",
        "0:v:0",
        "-t",
    ]
    .iter()
    .map(|s| s.to_string())
    .chain([secs.to_string()])
    .chain(
        ["-f", "null", "-", "-progress", "pipe:1"]
            .iter()
            .map(|s| s.to_string()),
    )
    .collect()
}

/// Parses ffmpeg `-progress` key=value output (last values win).
pub fn parse_ffmpeg_progress(text: &str) -> TestReport {
    let mut r = TestReport::default();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim();
        match k.trim() {
            "frame" => r.frames = v.parse().unwrap_or(r.frames),
            "fps" => r.fps = v.parse().unwrap_or(r.fps),
            "out_time_us" | "out_time_ms" => {
                // Both keys carry microseconds in current ffmpeg releases.
                if let Ok(us) = v.parse::<f64>() {
                    r.media_secs = us / 1_000_000.0;
                }
            }
            "speed" => r.speed = v.trim_end_matches('x').parse().unwrap_or(r.speed),
            _ => {}
        }
    }
    r
}

/// Resolves the tuner for a saved channel: by delivery, preferring the model
/// that found it. The stored `/dev/dvb/adapterN` path is not trusted because
/// adapter numbers follow plug order.
pub fn resolve_watch_device(ch: &SavedChannel, discovery: &Discovery) -> anyhow::Result<String> {
    let cat = delivery_category(&ch.delivery);
    discovery
        .tuner_for(cat, ch.tuner_model.as_deref())
        .map(|d| d.capture_address(cat))
        .ok_or_else(|| anyhow::anyhow!("no {} tuner found for '{}'", ch.delivery.as_str(), ch.name))
}

/// `tsp` arguments (without `-O`) that tune `ch` and zap to its service.
pub fn watch_tsp_args(
    ch: &SavedChannel,
    device_name: &str,
    signal_timeout_s: u32,
) -> anyhow::Result<Vec<String>> {
    let params = ch.to_capture_params(device_name.to_string(), signal_timeout_s)?;
    let mut args = params.to_tsp_args();
    if let Some(i) = args.iter().position(|a| a == "-O") {
        args.truncate(i);
    }
    args.extend(["-P".into(), "zap".into(), ch.service_id.to_string()]);
    Ok(args)
}

/// Human-readable pipeline for `--dry-run` and logs.
pub fn describe_pipeline(tsp_args: &[String], player: &[String]) -> String {
    format!(
        "{} -O file | {}",
        tsduck::display_command("tsp", tsp_args),
        player.join(" ")
    )
}

/// Watches a saved channel.
pub fn watch_channel(
    paths: &TsduckPaths,
    ch: &SavedChannel,
    discovery: &Discovery,
    opts: &WatchOpts,
) -> anyhow::Result<WatchEnd> {
    let device = resolve_watch_device(ch, discovery)?;
    let tsp_args = watch_tsp_args(ch, &device, WATCH_SIGNAL_TIMEOUT_S)?;
    let mut opts = opts.clone();
    opts.title.get_or_insert_with(|| ch.name.clone());
    eprintln!(
        "[dvbhub] Watching '{}' (sid {}) on {device} — {}",
        ch.name,
        ch.service_id,
        ch.tune_summary()
    );
    run_watch(paths, &tsp_args, &opts)
}

/// Runs `tsp` piped into the player until either side exits.
pub fn run_watch(
    paths: &TsduckPaths,
    tsp_args: &[String],
    opts: &WatchOpts,
) -> anyhow::Result<WatchEnd> {
    let t0 = Instant::now();
    let player = player_argv(opts)?;
    let pipeline = describe_pipeline(tsp_args, &player);
    lastlog::event("watch", &pipeline);
    eprintln!("[dvbhub] {pipeline}");
    eprintln!("[dvbhub] log → {}", lastlog::path().display());

    install_interrupt_handler();
    let tsp_bin = paths.tool("tsp");
    let mut tsp = die_with_parent(&mut Command::new(&tsp_bin))
        .arg("--verbose")
        .args(tsp_args)
        .args(["-O", "file"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start {}", tsp_bin.display()))?;
    let tsp_out = tsp.stdout.take().context("tsp stdout")?;
    let last_error = Arc::new(Mutex::new(None::<String>));
    let tsp_log =
        spawn_tsp_stderr_reader(tsp.stderr.take().context("tsp stderr")?, t0, &last_error);

    let mut player_child = die_with_parent(&mut Command::new(&player[0]))
        .args(&player[1..])
        .stdin(Stdio::from(tsp_out))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start player {}", player[0]))?;
    let progress = Arc::new(Mutex::new(String::new()));
    let player_logs = spawn_player_readers(&mut player_child, &progress, opts.test_secs.is_some());

    let (user_closed, tsp_status) = supervise(&mut tsp, &mut player_child)?;
    let _ = tsp_log.join();
    for h in player_logs {
        let _ = h.join();
    }

    let tsp_error = match tsp_status {
        Some(status) if !status.success() => Some(
            last_error
                .lock()
                .ok()
                .and_then(|e| e.clone())
                .unwrap_or_else(|| format!("tsp exited with {status}")),
        ),
        _ => None,
    };
    let test = opts.test_secs.map(|_| {
        let mut report = progress
            .lock()
            .map(|t| parse_ffmpeg_progress(&t))
            .unwrap_or_default();
        report.decode_errors = report.decode_errors.max(count_lines(&progress, "error"));
        report
    });
    lastlog::event_elapsed(
        "watch",
        t0,
        &format!("done user_closed={user_closed} tsp_error={tsp_error:?} test={test:?}"),
    );
    Ok(WatchEnd {
        user_closed,
        tsp_error,
        test,
    })
}

/// Waits for the first of the two processes to exit and stops the other.
/// Returns `(player_closed_first, tsp_exit_status_if_tsp_ended_first)`.
fn supervise(
    tsp: &mut Child,
    player: &mut Child,
) -> anyhow::Result<(bool, Option<std::process::ExitStatus>)> {
    loop {
        if INTERRUPTED.load(Ordering::SeqCst) {
            stop_child(player);
            stop_child(tsp);
            return Ok((true, None));
        }
        if player.try_wait()?.is_some() {
            stop_child(tsp);
            return Ok((true, None));
        }
        if let Some(status) = tsp.try_wait()? {
            // Let the player drain what is buffered (ffmpeg finishes its window,
            // mpv reaches EOF), then make sure it is gone.
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline && player.try_wait()?.is_none() {
                thread::sleep(Duration::from_millis(100));
            }
            stop_child(player);
            return Ok((false, Some(status)));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// SIGTERM, then SIGKILL after a grace period.
fn stop_child(child: &mut Child) {
    if matches!(child.try_wait(), Ok(Some(_))) {
        return;
    }
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
    }
    let deadline = Instant::now() + TSP_STOP_GRACE;
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Logs `tsp` stderr, echoes lock milestones and remembers the last error.
fn spawn_tsp_stderr_reader(
    stderr: impl Read + Send + 'static,
    t0: Instant,
    last_error: &Arc<Mutex<Option<String>>>,
) -> JoinHandle<()> {
    let last_error = Arc::clone(last_error);
    thread::spawn(move || {
        let mut locked = false;
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let line = line.trim().to_string();
            lastlog::event_elapsed("tsp", t0, &line);
            if !locked && line.contains("locked: yes") {
                locked = true;
                eprintln!("[dvbhub] +{:.1}s {line}", t0.elapsed().as_secs_f32());
            }
            if line.contains("Error") || line.contains("error:") {
                eprintln!("[dvbhub] tsp: {line}");
                if let Ok(mut e) = last_error.lock() {
                    *e = Some(line);
                }
            }
        }
    })
}

/// Collects player stdout (ffmpeg progress in test mode) and logs stderr.
fn spawn_player_readers(
    player: &mut Child,
    progress: &Arc<Mutex<String>>,
    test: bool,
) -> Vec<JoinHandle<()>> {
    let mut handles = Vec::new();
    if let Some(out) = player.stdout.take() {
        let progress = Arc::clone(progress);
        handles.push(thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                lastlog::event("player", line.trim());
                if test {
                    if let Ok(mut p) = progress.lock() {
                        p.push_str(&line);
                        p.push('\n');
                    }
                }
            }
        }));
    }
    if let Some(err) = player.stderr.take() {
        let progress = Arc::clone(progress);
        handles.push(thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                lastlog::event("player", line.trim());
                if test {
                    if let Ok(mut p) = progress.lock() {
                        p.push_str("stderr: ");
                        p.push_str(&line);
                        p.push('\n');
                    }
                }
            }
        }));
    }
    handles
}

fn count_lines(text: &Arc<Mutex<String>>, needle: &str) -> usize {
    text.lock()
        .map(|t| {
            t.lines()
                .filter(|l| l.starts_with("stderr:") && l.to_ascii_lowercase().contains(needle))
                .count()
        })
        .unwrap_or(0)
}

fn delivery_category(d: &ChannelDelivery) -> DeliveryCategory {
    match d {
        ChannelDelivery::Satellite => DeliveryCategory::Satellite,
        ChannelDelivery::Cable => DeliveryCategory::Cable,
        ChannelDelivery::Terrestrial => DeliveryCategory::Terrestrial,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ffmpeg_progress_last_block() {
        let text = "frame=10\nfps=0.0\nout_time_us=400000\nspeed=N/A\nprogress=continue\n\
                    frame=250\nfps=25.01\nout_time_us=10000000\nspeed=1.00x\nprogress=end\n";
        let r = parse_ffmpeg_progress(text);
        assert_eq!(r.frames, 250);
        assert!((r.fps - 25.01).abs() < 1e-9);
        assert!((r.media_secs - 10.0).abs() < 1e-9);
        assert!((r.speed - 1.0).abs() < 1e-9);
        assert!(r.passed(10));
    }

    #[test]
    fn report_fails_without_frames_or_when_short() {
        let none = TestReport::default();
        assert!(!none.passed(10));
        let short = TestReport {
            frames: 50,
            media_secs: 2.0,
            speed: 1.0,
            ..Default::default()
        };
        assert!(!short.passed(10));
    }

    #[test]
    fn ffmpeg_test_argv_limits_duration() {
        let argv = ffmpeg_test_argv(12);
        let t = argv.iter().position(|a| a == "-t").unwrap();
        assert_eq!(argv[t + 1], "12");
        assert!(argv.contains(&"pipe:1".to_string()));
    }
}

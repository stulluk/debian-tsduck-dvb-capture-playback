//! Simple piped watch: `tsp stdout → mpv` or `tsp stdout → gst-launch-1.0`.
//!
//! Same model as the working one-liner; GStreamer owns demux/decode/PCR sync internally.

use crate::lastlog;
use crate::tools::{self, find_executable_on_path};
use crate::watch_gst::{WatchAudioMode, WatchEnd};
use anyhow::Context;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipeBackend {
    /// `tsp … -O file | mpv -` — best PCR/A-V sync (recommended).
    Mpv,
    /// `tsp … -O file | gst-launch-1.0 fdsrc ! tsdemux ! decodebin ! …`
    GstLaunch,
}

impl PipeBackend {
    pub fn from_env() -> Self {
        match std::env::var("DVBHUB_WATCH_BACKEND")
            .ok()
            .map(|v| v.to_ascii_lowercase())
            .as_deref()
        {
            Some("embed" | "embedded" | "appsrc") => {
                // Handled in watch.rs — should not reach here.
                Self::GstLaunch
            }
            Some("gst" | "gst-launch" | "gstreamer") => Self::GstLaunch,
            _ => Self::Mpv,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Mpv => "mpv (pipe)",
            Self::GstLaunch => "gst-launch-1.0 (pipe)",
        }
    }
}

pub fn pipeline_description(backend: PipeBackend, headless: bool, audio: WatchAudioMode) -> String {
    match backend {
        PipeBackend::Mpv => {
            if headless {
                "tsp -O file | mpv --vo=null --ao=null -".into()
            } else {
                "tsp -O file | mpv --force-window=yes --title=dvbhub --cache=yes -".into()
            }
        }
        PipeBackend::GstLaunch => gst_launch_summary(headless, audio),
    }
}

fn gst_launch_summary(headless: bool, audio: WatchAudioMode) -> String {
    let (v, a) = if headless {
        ("fakesink sync=true", "fakesink sync=true")
    } else {
        ("autovideosink sync=true", "autoaudiosink sync=true")
    };
    let audio_arm = if audio == WatchAudioMode::None {
        String::new()
    } else {
        format!(" d. ! queue ! decodebin ! {a}")
    };
    format!(
        "tsp -O file | gst-launch-1.0 -e fdsrc ! queue ! tsdemux name=d d. ! queue ! decodebin ! {v}{audio_arm}"
    )
}

/// Resolve `gst-launch-1.0` on `$PATH` (and common install locations).
pub fn find_gst_launch() -> Option<PathBuf> {
    if let Some(p) = find_executable_on_path("gst-launch-1.0") {
        return Some(p);
    }
    for p in ["/usr/bin/gst-launch-1.0", "/usr/local/bin/gst-launch-1.0"] {
        let path = Path::new(p);
        if tools::is_executable(path) {
            return Some(path.to_path_buf());
        }
    }
    None
}

pub fn ensure_gst_launch() -> anyhow::Result<PathBuf> {
    find_gst_launch().ok_or_else(|| {
        anyhow::anyhow!(
            "gst-launch-1.0 not found — install on the host:\n\
             apt install gstreamer1.0-tools gstreamer1.0-plugins-base \
             gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-libav\n\
             Or use the default backend: DVBHUB_WATCH_BACKEND=mpv"
        )
    })
}

pub struct PipedPlayerWatch {
    tsp: Child,
    player: Child,
    stop: Arc<AtomicBool>,
    handles: Vec<thread::JoinHandle<()>>,
    backend: PipeBackend,
}

impl PipedPlayerWatch {
    pub fn spawn(
        backend: PipeBackend,
        tsp_bin: &Path,
        tsp_args: &[String],
        headless: bool,
        audio: WatchAudioMode,
        t0: Instant,
    ) -> anyhow::Result<Self> {
        let mut tsp = Command::new(tsp_bin)
            .arg("--verbose")
            .args(tsp_args)
            .arg("-O")
            .arg("file")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawn tsp {}", tsp_bin.display()))?;
        lastlog::event_elapsed("pipe", t0, &format!("tsp pid={}", tsp.id()));

        let tsp_stdout = tsp.stdout.take().context("tsp stdout")?;
        let tsp_stderr = tsp.stderr.take().context("tsp stderr")?;

        let stop = Arc::new(AtomicBool::new(false));

        let (player, mut extra_handles) = match backend {
            PipeBackend::Mpv => spawn_mpv(tsp_stdout, headless, t0)?,
            PipeBackend::GstLaunch => {
                spawn_gst_launch(tsp_stdout, headless, audio, Arc::clone(&stop), t0)?
            }
        };

        let mut handles = Vec::new();
        {
            let stop_tsp = Arc::clone(&stop);
            handles.push(thread::spawn(move || tail_tsp_stderr(tsp_stderr, stop_tsp, t0)));
        }
        handles.append(&mut extra_handles);

        Ok(Self {
            tsp,
            player,
            stop,
            handles,
            backend,
        })
    }

    pub fn wait(mut self, t0: Instant, max_duration: Option<Duration>) -> anyhow::Result<WatchEnd> {
        let status = if let Some(limit) = max_duration {
            let deadline = Instant::now() + limit;
            loop {
                match self.player.try_wait().context("try_wait player")? {
                    Some(status) => break status,
                    None if Instant::now() >= deadline => {
                        lastlog::event_elapsed(
                            "watch.test",
                            t0,
                            &format!("duration limit {:.0}s — stopping", limit.as_secs_f64()),
                        );
                        let _ = self.player.kill();
                        break self.player.wait().context("wait player after kill")?;
                    }
                    None => thread::sleep(Duration::from_millis(200)),
                }
            }
        } else {
            self.player.wait().context("wait player")?
        };

        self.stop.store(true, Ordering::Relaxed);
        let _ = self.tsp.kill();
        let _ = self.tsp.wait();
        for h in self.handles {
            let _ = h.join();
        }

        let code = status.code().unwrap_or(-1);
        let ok = status.success();
        lastlog::event_elapsed(
            "pipe",
            t0,
            &format!("{} exit ok={ok} code={code}", self.backend.label()),
        );

        // mpv/gst exit 0 after closing the video window — treat as clean TUI return.
        let user_closed_video = ok && max_duration.is_none();
        Ok(WatchEnd {
            ok,
            user_closed_video,
        })
    }
}

fn spawn_mpv(
    tsp_stdout: impl Into<Stdio>,
    headless: bool,
    t0: Instant,
) -> anyhow::Result<(Child, Vec<thread::JoinHandle<()>>)> {
    let mut args: Vec<String> = vec![
        "--cache=yes".into(),
        "--demuxer-readahead-secs=0".into(),
        "--demuxer=lavf".into(),
        "--demuxer-lavf-format=mpegts".into(),
    ];
    if headless {
        args.extend([
            "--vo=null".into(),
            "--ao=null".into(),
            "--really-quiet".into(),
        ]);
    } else {
        args.extend([
            "--force-window=yes".into(),
            "--title=dvbhub".into(),
        ]);
    }
    args.push("-".into());

    let mut cmd = Command::new("mpv");
    cmd.args(&args)
        .stdin(tsp_stdout)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut player = cmd
        .spawn()
        .context("spawn mpv — install: apt install mpv")?;
    lastlog::event_elapsed("pipe", t0, &format!("mpv pid={}", player.id()));

    let mut handles = Vec::new();
    if let Some(stderr) = player.stderr.take() {
        handles.push(thread::spawn(move || tail_mpv_stderr(stderr)));
    }
    Ok((player, handles))
}

fn spawn_gst_launch(
    tsp_stdout: impl Into<Stdio>,
    headless: bool,
    audio: WatchAudioMode,
    stop: Arc<AtomicBool>,
    t0: Instant,
) -> anyhow::Result<(Child, Vec<thread::JoinHandle<()>>)> {
    let gst_bin = ensure_gst_launch()?;
    let mut args = gst_launch_argv(headless, audio);
    lastlog::event("pipe.gst", &args.join(" "));

    let mut player = Command::new(&gst_bin)
        .args(&args)
        .stdin(tsp_stdout)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawn {}", gst_bin.display()))?;
    lastlog::event_elapsed(
        "pipe",
        t0,
        &format!("{} pid={}", gst_bin.display(), player.id()),
    );

    if let Some(stderr) = player.stderr.take() {
        let h = thread::spawn(move || tail_gst_stderr(stderr, stop));
        return Ok((player, vec![h]));
    }

    Ok((player, Vec::new()))
}

/// gst-launch argv: `-e fdsrc … ! tsdemux name=d d. ! …`
fn gst_launch_argv(headless: bool, audio: WatchAudioMode) -> Vec<String> {
    let mut args = vec!["-e".into()];
    args.extend([
        "fdsrc".into(),
        "fd=0".into(),
        "blocksize=12032".into(),
        "do-timestamp=true".into(),
        "!".into(),
        "queue".into(),
        "leaky=downstream".into(),
        "max-size-bytes=4194304".into(),
        "!".into(),
        "tsdemux".into(),
        "name=d".into(),
        "d.".into(),
        "!".into(),
        "queue".into(),
        "!".into(),
        "decodebin".into(),
        "!".into(),
    ]);
    if headless {
        args.extend(["fakesink".into(), "sync=true".into()]);
    } else {
        args.extend(["autovideosink".into(), "sync=true".into()]);
    }
    if audio != WatchAudioMode::None {
        args.extend([
            "d.".into(),
            "!".into(),
            "queue".into(),
            "!".into(),
            "decodebin".into(),
            "!".into(),
        ]);
        if headless {
            args.extend(["fakesink".into(), "sync=true".into()]);
        } else {
            args.extend(["autoaudiosink".into(), "sync=true".into()]);
        }
    }
    args
}

fn tail_tsp_stderr(stderr: impl std::io::Read, stop: Arc<AtomicBool>, t0: Instant) {
    let reader = BufReader::new(stderr);
    for line in reader.lines().map_while(Result::ok) {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        lastlog::event("tsp", &line);
        log_tsp_milestone(t0, &line);
    }
}

fn log_tsp_milestone(t0: Instant, line: &str) {
    let tag = if line.contains("using /dev/dvb") {
        Some("tuner open")
    } else if line.contains("locked: yes") {
        Some("RF locked")
    } else if line.contains("initial input bitrate") {
        Some("TS flowing")
    } else if line.contains("zap: found service") {
        Some("zap ready")
    } else {
        None
    };
    if let Some(tag) = tag {
        lastlog::event_elapsed("watch", t0, &format!("{tag}: {}", line.trim()));
        eprintln!("[dvbhub] +{:.1}s {tag}", t0.elapsed().as_secs_f32());
    }
}

fn tail_mpv_stderr(stderr: impl std::io::Read) {
    let reader = BufReader::new(stderr);
    for line in reader.lines().map_while(Result::ok) {
        if line.contains("error") || line.contains("Error") || line.contains("● Video") {
            lastlog::event("mpv", &line);
        }
    }
}

fn tail_gst_stderr(stderr: impl std::io::Read, stop: Arc<AtomicBool>) {
    let reader = BufReader::new(stderr);
    for line in reader.lines().map_while(Result::ok) {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let interesting = line.contains("ERROR")
            || line.contains("WARN")
            || line.contains("Playing")
            || line.contains("Prerolled")
            || line.contains("Redistribute");
        if interesting {
            lastlog::event("gst.pipe", &line);
            if line.contains("ERROR") {
                eprintln!("[dvbhub] gst {line}");
            }
        }
    }
}

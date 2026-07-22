//! Live watch pipeline: tuner → zap → player (pipe).

use crate::channels::{ChannelDelivery, SavedChannel};
use crate::config::TsduckPaths;
use crate::devices::{DeliveryCategory, Discovery};
use crate::lastlog;
use crate::tools;
use crate::tsduck;
use crate::watch_gst::{self, GstOpts, GstWatch, WatchAudioMode, WatchEnd};
use crate::watch_pipe::{self, PipeBackend, PipedPlayerWatch};
use std::path::Path;
use std::time::Instant;

pub const WATCH_SIGNAL_TIMEOUT_S: u32 = 10;

/// Which playback engine runs after `tsp -P zap`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchBackend {
    /// `tsp | mpv -` — default; PCR/A-V sync like your one-liner.
    Mpv,
    /// `tsp | gst-launch-1.0 fdsrc ! tsdemux ! decodebin ! …`
    GstPipe,
    /// In-process appsrc + manual demux (debug / --sync-test only).
    Embed,
}

impl WatchBackend {
    pub fn from_env(sync_diag: bool) -> Self {
        if sync_diag {
            return Self::Embed;
        }
        match std::env::var("DVBHUB_WATCH_BACKEND")
            .ok()
            .map(|v| v.to_ascii_lowercase())
            .as_deref()
        {
            Some("embed" | "embedded" | "appsrc") => Self::Embed,
            Some("gst" | "gst-launch" | "gstreamer") => Self::GstPipe,
            Some("mpv") => Self::Mpv,
            _ => Self::Mpv,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Mpv => "mpv",
            Self::GstPipe => "gst-launch",
            Self::Embed => "gstreamer (embedded)",
        }
    }
}

pub fn default_watch_player_cmd() -> anyhow::Result<String> {
    ensure_watch_backend(WatchBackend::from_env(false))?;
    match WatchBackend::from_env(false) {
        WatchBackend::Embed => Ok("gstreamer (embedded)".into()),
        WatchBackend::GstPipe => Ok(format!(
            "{} (pipe)",
            watch_pipe::find_gst_launch()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "gst-launch-1.0".into())
        )),
        WatchBackend::Mpv => Ok("mpv (pipe)".into()),
    }
}

/// Validate backend dependencies before building the pipeline (fail fast in TUI).
pub fn ensure_watch_backend(backend: WatchBackend) -> anyhow::Result<()> {
    match backend {
        WatchBackend::Mpv => {
            tools::ensure_watch_player("mpv")?;
        }
        WatchBackend::GstPipe => {
            watch_pipe::ensure_gst_launch()?;
        }
        WatchBackend::Embed => {
            watch_gst::ensure_gstreamer()?;
        }
    }
    Ok(())
}

pub fn resolve_watch_device(ch: &SavedChannel, discovery: &Discovery) -> anyhow::Result<String> {
    if let Some(name) = &ch.device_name {
        if name.starts_with("/dev/") || name.contains(':') {
            return Ok(name.clone());
        }
    }
    let cat = delivery_category(&ch.delivery);
    discovery
        .tuner_for_category(cat)
        .map(|d| d.capture_address(cat))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no {} tuner found for '{}'",
                ch.delivery.as_str(),
                ch.name
            )
        })
}

pub fn watch_tsp_dvb_args(
    ch: &SavedChannel,
    device_name: &str,
    signal_timeout_s: u32,
) -> anyhow::Result<Vec<String>> {
    let params = ch.to_capture_params(device_name.to_string(), signal_timeout_s)?;
    let mut tsp_args = params.to_tsp_args();
    if let Some(i) = tsp_args.iter().position(|a| a == "-O") {
        tsp_args.truncate(i);
    }
    tsp_args.extend([
        "-P".into(),
        "zap".into(),
        ch.service_id.to_string(),
    ]);
    Ok(tsp_args)
}

/// Legacy display helper (dry-run).
pub fn build_fork_cmd(_player_cmd: &str, _transcode_audio: bool) -> String {
    pipe_pipeline_description(false, WatchAudioMode::from_env())
}

fn pipe_pipeline_description(headless: bool, audio: WatchAudioMode) -> String {
    let backend = match WatchBackend::from_env(false) {
        WatchBackend::Mpv => PipeBackend::Mpv,
        WatchBackend::GstPipe => PipeBackend::GstLaunch,
        WatchBackend::Embed => {
            return watch_gst::pipeline_description(headless);
        }
    };
    watch_pipe::pipeline_description(backend, headless, audio)
}

fn frontend_path(device: &str) -> Option<std::path::PathBuf> {
    if device.contains(":0") || device.contains(":1") {
        return None;
    }
    let base = device.trim_end_matches('/').trim();
    if base.starts_with("/dev/dvb/adapter") {
        return Some(Path::new(base).join("frontend0"));
    }
    None
}

/// Options for piped watch (interactive vs headless test).
pub struct WatchOpts {
    pub headless: bool,
    pub duration_secs: Option<u32>,
    pub transcode_audio: bool,
    /// Log PCR vs PTS vs wall-clock drift (forces embedded backend).
    pub sync_diag: bool,
    pub audio: WatchAudioMode,
}

impl Default for WatchOpts {
    fn default() -> Self {
        Self {
            headless: false,
            duration_secs: None,
            transcode_audio: false,
            sync_diag: false,
            audio: WatchAudioMode::from_env(),
        }
    }
}

/// Outcome of a TUI watch session (window close is not an error).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchTuiOutcome {
    Finished,
    UserClosedVideo,
}

pub fn run_watch(
    paths: &TsduckPaths,
    ch: &SavedChannel,
    discovery: &Discovery,
    _player_cmd: Option<&str>,
    transcode_audio: bool,
) -> anyhow::Result<()> {
    run_watch_for_tui(paths, ch, discovery, transcode_audio).map(|_| ())
}

/// Watch from the TUI: closing the video window returns to the channel list cleanly.
pub fn run_watch_for_tui(
    paths: &TsduckPaths,
    ch: &SavedChannel,
    discovery: &Discovery,
    transcode_audio: bool,
) -> anyhow::Result<WatchTuiOutcome> {
    let end = run_watch_with_opts(
        paths,
        ch,
        discovery,
        WatchOpts {
            transcode_audio,
            ..Default::default()
        },
    )?;
    if end.user_closed_video {
        Ok(WatchTuiOutcome::UserClosedVideo)
    } else {
        Ok(WatchTuiOutcome::Finished)
    }
}

pub fn run_watch_with_opts(
    paths: &TsduckPaths,
    ch: &SavedChannel,
    discovery: &Discovery,
    opts: WatchOpts,
) -> anyhow::Result<WatchEnd> {
    let t0 = Instant::now();
    let backend = WatchBackend::from_env(opts.sync_diag);
    ensure_watch_backend(backend)?;
    let device = resolve_watch_device(ch, discovery)?;
    let tsp_args = watch_tsp_dvb_args(ch, &device, WATCH_SIGNAL_TIMEOUT_S)?;

    lastlog::event_elapsed(
        "watch",
        t0,
        &format!(
            "start channel={:?} sid={} tune={} device={device} backend={}",
            ch.name,
            ch.service_id,
            ch.tune_summary(),
            backend.label()
        ),
    );
    if let Some(fe) = frontend_path(&device) {
        let holder = lastlog::frontend_holder(&fe);
        lastlog::event(
            "adapter",
            &format!(
                "frontend={} busy={} holder={holder}",
                fe.display(),
                !holder.is_empty()
            ),
        );
    }

    eprintln!(
        "[dvbhub] Watching '{}' (sid {}) on {} — tsp → {}",
        ch.name, ch.service_id, device, backend.label()
    );
    eprintln!("[dvbhub] log → {}", lastlog::path().display());
    eprintln!("[dvbhub] +0.0s waiting for RF lock…");

    let pipe_desc = pipe_pipeline_description(opts.headless, opts.audio);
    let cmd_line = format!(
        "{} -O file | {}",
        tsduck::display_command("tsp", &tsp_args),
        pipe_desc
    );
    lastlog::event("watch.tsp", &cmd_line);

    let tsp_bin = paths.tool("tsp");
    let max_duration = opts
        .duration_secs
        .map(|s| std::time::Duration::from_secs(u64::from(s)));

    let end = match backend {
        WatchBackend::Embed => {
            let gst = GstWatch::spawn(
                &tsp_bin,
                &tsp_args,
                GstOpts {
                    headless: opts.headless,
                    transcode_audio: opts.transcode_audio,
                    sync_diag: opts.sync_diag,
                    audio: opts.audio,
                },
                t0,
            )?;
            gst.wait(t0, max_duration)?
        }
        WatchBackend::Mpv => {
            let pipe = PipedPlayerWatch::spawn(
                PipeBackend::Mpv,
                &tsp_bin,
                &tsp_args,
                opts.headless,
                opts.audio,
                t0,
            )?;
            pipe.wait(t0, max_duration)?
        }
        WatchBackend::GstPipe => {
            let pipe = PipedPlayerWatch::spawn(
                PipeBackend::GstLaunch,
                &tsp_bin,
                &tsp_args,
                opts.headless,
                opts.audio,
                t0,
            )?;
            pipe.wait(t0, max_duration)?
        }
    };

    lastlog::event_elapsed(
        "watch",
        t0,
        &format!(
            "done ok={} user_closed={}",
            end.success(),
            end.user_closed_video
        ),
    );
    Ok(end)
}

/// Manual watch from explicit `tsp` args (transponder + `-P zap <sid>`).
pub fn run_tsp_watch(
    paths: &TsduckPaths,
    tsp_args: Vec<String>,
    opts: WatchOpts,
) -> anyhow::Result<WatchEnd> {
    let t0 = Instant::now();
    let backend = WatchBackend::from_env(opts.sync_diag);
    ensure_watch_backend(backend)?;
    lastlog::event("watch", &format!("manual backend={}", backend.label()));
    eprintln!(
        "[dvbhub] tsp → {} — log: {}",
        backend.label(),
        lastlog::path().display()
    );
    let tsp_bin = paths.tool("tsp");
    let max_duration = opts
        .duration_secs
        .map(|s| std::time::Duration::from_secs(u64::from(s)));
    match backend {
        WatchBackend::Embed => {
            GstWatch::spawn(
                &tsp_bin,
                &tsp_args,
                GstOpts {
                    headless: opts.headless,
                    transcode_audio: opts.transcode_audio,
                    sync_diag: opts.sync_diag,
                    audio: opts.audio,
                },
                t0,
            )?
            .wait(t0, max_duration)
        }
        WatchBackend::Mpv => {
            PipedPlayerWatch::spawn(
                PipeBackend::Mpv,
                &tsp_bin,
                &tsp_args,
                opts.headless,
                opts.audio,
                t0,
            )?
            .wait(t0, max_duration)
        }
        WatchBackend::GstPipe => PipedPlayerWatch::spawn(
            PipeBackend::GstLaunch,
            &tsp_bin,
            &tsp_args,
            opts.headless,
            opts.audio,
            t0,
        )?
        .wait(t0, max_duration),
    }
}

fn delivery_category(d: &ChannelDelivery) -> DeliveryCategory {
    match d {
        ChannelDelivery::Satellite => DeliveryCategory::Satellite,
        ChannelDelivery::Cable => DeliveryCategory::Cable,
        ChannelDelivery::Terrestrial => DeliveryCategory::Terrestrial,
    }
}

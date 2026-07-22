//! GStreamer watch pipeline: `tsp` stdout → appsrc ! tsdemux ! decode → sinks.
//!
//! Replaces the external ffmpeg|mpv subprocess chain with an in-process GStreamer
//! pipeline for better debuggability (bus messages, pad probes, unified lastlog).

use crate::lastlog;
use crate::ts_pcr::{self, PcrState};
use anyhow::Context;
use gstreamer as gst;
use gstreamer_app as gst_app;
use gstreamer::prelude::*;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Whether to decode/play audio in the watch pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchAudioMode {
    /// Route audio through PulseAudio/PipeWire (async; must not block video).
    Auto,
    /// Video only — skip audio demux branch (workaround for broken PipeWire/ALSA).
    None,
}

impl WatchAudioMode {
    /// `DVBHUB_AUDIO=none|null|0` disables audio; anything else is Auto.
    pub fn from_env() -> Self {
        match std::env::var("DVBHUB_AUDIO")
            .ok()
            .map(|v| v.to_ascii_lowercase())
            .as_deref()
        {
            Some("none" | "null" | "0" | "no" | "false") => Self::None,
            _ => Self::Auto,
        }
    }
}

/// Runtime options for the GStreamer watch pipeline.
pub struct GstOpts {
    pub headless: bool,
    /// Kept for CLI compatibility; GStreamer decodes audio natively.
    pub transcode_audio: bool,
    /// Log PCR vs video PTS vs wall clock every second.
    pub sync_diag: bool,
    pub audio: WatchAudioMode,
}

/// Shared counters updated from pad probes and the TS feeder thread.
#[derive(Clone, Default)]
pub struct GstStats {
    pub ts_bytes: Arc<AtomicU64>,
    pub video_buffers: Arc<AtomicU64>,
    pub audio_buffers: Arc<AtomicU64>,
    pub last_video_pts_ns: Arc<AtomicU64>,
    pub first_video_pts_ns: Arc<AtomicU64>,
    pub pcr: PcrState,
}

/// How a watch session ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchEnd {
    pub ok: bool,
    /// User closed the GStreamer video window (X button).
    pub user_closed_video: bool,
}

/// Live watch session: tsp subprocess + GStreamer pipeline.
pub struct GstWatch {
    pipeline: gst::Pipeline,
    tsp: Child,
    stop: Arc<AtomicBool>,
    handles: Vec<thread::JoinHandle<()>>,
}

impl WatchEnd {
    pub fn success(self) -> bool {
        self.ok || self.user_closed_video
    }
}

/// Initializes GStreamer (idempotent). Call before spawning a pipeline.
pub fn ensure_gstreamer() -> anyhow::Result<()> {
    prepend_mpegtslive_plugin_path();
    gst::init().context(
        "GStreamer not available — on Debian: \
         apt install gstreamer1.0-plugins-base gstreamer1.0-plugins-good \
         gstreamer1.0-plugins-bad gstreamer1.0-libav gstreamer1.0-gl \
         (optional PCR clock: build/gst-plugins via docker/gst-mpegtslive/)",
    )?;
    Ok(())
}

/// True when `libgstmpegtslive.so` is on `GST_PLUGIN_PATH` (PCR pipeline clock).
pub fn mpegts_livesrc_available() -> bool {
    gst::ElementFactory::find("mpegtslivesrc").is_some()
}

/// `DVBHUB_MPEGTS_LIVE=1` enables mpegtslivesrc PCR pipeline clock (experimental).
fn mpegts_livesrc_enabled() -> bool {
    matches!(
        std::env::var("DVBHUB_MPEGTS_LIVE")
            .ok()
            .map(|v| v.to_ascii_lowercase())
            .as_deref(),
        Some("1" | "yes" | "true" | "on")
    )
}

/// Human-readable pipeline summary for dry-run / lastlog.
/// Safe without `gst::init()` — does not probe plugins.
pub fn pipeline_description(headless: bool) -> String {
    let sink = if headless { "identity sync=false" } else { "xvimagesink sync=false" };
    format!("appsrc ! queue ! tsdemux(ignore-pcr) ! queue ! {{h264,h265}}parse ! avdec ! {sink}")
}

fn prepend_mpegtslive_plugin_path() {
    let dir = mpegtslive_plugin_dir();
    if !dir.join("libgstmpegtslive.so").is_file() {
        return;
    }
    let dir_s = dir.display().to_string();
    let merged = match std::env::var("GST_PLUGIN_PATH") {
        Ok(existing) if !existing.is_empty() => format!("{dir_s}:{existing}"),
        _ => dir_s,
    };
    // SAFETY: called before gst::init(), single-threaded startup.
    unsafe { std::env::set_var("GST_PLUGIN_PATH", merged) };
}

fn mpegtslive_plugin_dir() -> std::path::PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let next_to_bin = parent.join("gst-plugins");
            if next_to_bin.join("libgstmpegtslive.so").is_file() {
                return next_to_bin;
            }
        }
    }
    std::path::PathBuf::from("build/gst-plugins")
}

fn configure_appsrc(appsrc: &gst_app::AppSrc) {
    let caps = gst::Caps::builder("video/mpegts")
        .field("systemstream", true)
        .field("packetsize", 188i32)
        .build();
    appsrc.set_caps(Some(&caps));
    appsrc.set_property("is-live", true);
    appsrc.set_property("format", gst::Format::Bytes);
    // Non-blocking: avoid freezing tsp when GStreamer backs up briefly.
    appsrc.set_property("block", false);
    appsrc.set_property("max-bytes", 4_000_000u64);
}

fn make_leaky_queue() -> anyhow::Result<gst::Element> {
    Ok(gst::ElementFactory::make("queue")
        .property("max-size-buffers", 0u32)
        .property("max-size-time", 0u64)
        .property("max-size-bytes", 4_000_000u32)
        .property_from_str("leaky", "downstream")
        .build()
        .context("queue")?)
}

/// `mpegtslivesrc` owns the PCR pipeline clock; tsdemux must ignore PCR in-band
/// (see GStreamer mpegtslivesrc docs). Without livesrc, ignore-pcr is still required
/// for live push but there is no clock → jerky preview with sync=false sinks.
fn make_tsdemux(with_livesrc: bool) -> anyhow::Result<gst::Element> {
    let demux = gst::ElementFactory::make("tsdemux")
        .name("demux")
        .property("ignore-pcr", true)
        .build()
        .context("tsdemux")?;
    if with_livesrc {
        let _ = demux.set_property_from_str("skew-corrections", "false");
    }
    Ok(demux)
}

struct TsIngress {
    appsrc: gst_app::AppSrc,
    clock_src: Option<gst::Element>,
    mode_label: &'static str,
}

fn build_ts_ingress(pipeline: &gst::Pipeline) -> anyhow::Result<TsIngress> {
    let queue = make_leaky_queue()?;
    let use_live = mpegts_livesrc_available() && mpegts_livesrc_enabled();
    let demux = make_tsdemux(use_live)?;

    if use_live {
        let inner = gst::ElementFactory::make("appsrc")
            .name("ts_feed")
            .build()
            .context("appsrc")?;
        let livesrc = gst::ElementFactory::make("mpegtslivesrc")
            .name("ts_live")
            .build()
            .context("mpegtslivesrc")?;
        livesrc.set_property("source", inner.clone().upcast::<gst::Element>());

        pipeline
            .add_many([&livesrc, &queue, &demux])
            .context("add mpegtslivesrc chain")?;
        gst::Element::link_many([&livesrc, &queue, &demux]).context("link livesrc→demux")?;

        let appsrc = inner
            .downcast::<gst_app::AppSrc>()
            .map_err(|_| anyhow::anyhow!("appsrc downcast"))?;
        configure_appsrc(&appsrc);
        lastlog::event(
            "gst",
            "mpegtslivesrc + tsdemux(ignore-pcr, skew-corrections=false) — PCR pipeline clock",
        );
        Ok(TsIngress {
            appsrc,
            clock_src: Some(livesrc),
            mode_label: "mpegtslivesrc",
        })
    } else {
        let appsrc_el = gst::ElementFactory::make("appsrc")
            .name("ts_src")
            .build()
            .context("appsrc")?;
        pipeline
            .add_many([&appsrc_el, &queue, &demux])
            .context("add appsrc chain")?;
        gst::Element::link_many([&appsrc_el, &queue, &demux]).context("link appsrc→demux")?;
        let appsrc = appsrc_el
            .downcast::<gst_app::AppSrc>()
            .map_err(|_| anyhow::anyhow!("appsrc downcast"))?;
        configure_appsrc(&appsrc);
        lastlog::event(
            "gst",
            "fallback: no mpegtslivesrc plugin — tsdemux ignore-pcr=true (jerky live preview)",
        );
        Ok(TsIngress {
            appsrc,
            clock_src: None,
            mode_label: "appsrc-ignore-pcr",
        })
    }
}

fn adopt_pcr_clock(pipeline: &gst::Pipeline, clock_src: &gst::Element) -> bool {
    if let Some(clock) = clock_src.clock() {
        pipeline.use_clock(Some(&clock));
        lastlog::event("gst", &format!("pipeline clock from {}", clock_src.name()));
        return true;
    }
    false
}

fn try_adopt_pcr_clock_after_ts(
    pipeline: &gst::Pipeline,
    clock_src: &gst::Element,
    pcr: &PcrState,
    adopted: &AtomicBool,
) {
    if adopted.load(Ordering::Relaxed) || pcr.pcr_count.load(Ordering::Relaxed) < 30 {
        return;
    }
    for _ in 0..60 {
        if adopt_pcr_clock(pipeline, clock_src) {
            adopted.store(true, Ordering::Relaxed);
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// PCR-timed display: sink waits for pipeline clock (smooth), never drops as "too late".
fn make_pcr_video_sink() -> anyhow::Result<gst::Element> {
    if gst::ElementFactory::find("xvimagesink").is_some() {
        Ok(gst::ElementFactory::make("xvimagesink")
            .property("sync", true)
            .property("max-lateness", -1i64)
            .property("qos", false)
            .build()?)
    } else {
        Ok(gst::ElementFactory::make("autovideosink")
            .property("sync", true)
            .build()?)
    }
}

/// Audio output for interactive watch. Uses PulseAudio/PipeWire when available;
/// `async=true` so a broken sink cannot pull the whole pipeline to PAUSED.
fn make_audio_sink(pcr_sync: bool) -> anyhow::Result<gst::Element> {
    if gst::ElementFactory::find("pulsesink").is_some() {
        lastlog::event("gst", "audio sink: pulsesink (async, PulseAudio/PipeWire default)");
        return Ok(gst::ElementFactory::make("pulsesink")
            .property("sync", pcr_sync)
            .property("async", true)
            .build()?);
    }
    if gst::ElementFactory::find("alsasink").is_some() {
        let mut builder = gst::ElementFactory::make("alsasink")
            .property("sync", pcr_sync)
            .property("async", true);
        if let Ok(device) = std::env::var("DVBHUB_ALSA_DEVICE") {
            lastlog::event("gst", &format!("audio sink: alsasink device={device}"));
            builder = builder.property("device", device);
        } else {
            lastlog::event("gst", "audio sink: alsasink (async)");
        }
        return Ok(builder.build()?);
    }
    lastlog::event("gst", "audio sink: autoaudiosink (fallback)");
    Ok(gst::ElementFactory::make("autoaudiosink")
        .property("sync", pcr_sync)
        .property("async", true)
        .build()?)
}

/// Live preview without PCR clock — shows newest frame; sync=false so the window
/// appears even when the PCR pipeline clock is not adopted yet.
fn make_live_video_sink() -> anyhow::Result<gst::Element> {
    if gst::ElementFactory::find("xvimagesink").is_some() {
        Ok(gst::ElementFactory::make("xvimagesink")
            .property("sync", false)
            .property("async", true)
            .property("max-lateness", -1i64)
            .property("qos", false)
            .build()?)
    } else {
        Ok(gst::ElementFactory::make("autovideosink")
            .property("sync", false)
            .build()?)
    }
}

fn log_sink_choice(kind: &str, sink: &gst::Element) {
    let factory = sink.factory().map(|f| f.name().to_string()).unwrap_or_else(|| "?".into());
    let sync = sink.property::<bool>("sync");
    lastlog::event(
        "gst",
        &format!("{kind} sink: {factory} sync={sync}"),
    );
}

/// Link a new branch into a live PLAYING pipeline without pulling it back to PAUSED.
fn start_branch_playing(elements: &[gst::Element], t0: Instant, label: &'static str) {
    lastlog::event("gst", &format!("{label}: set_state(Playing) on branch"));
    eprintln!(
        "[dvbhub] gst {label}: start branch (+{:.1}s)",
        t0.elapsed().as_secs_f32()
    );
    for e in elements {
        let name = e.name();
        let ret = e.set_state(gst::State::Playing);
        lastlog::event("gst", &format!("{label}: {name} set_state(Playing) -> {ret:?}"));
    }
    lastlog::event_elapsed("gst", t0, &format!("{label} branch playing"));
}

fn is_user_closed_video_error(err: &gst::message::Error) -> bool {
    let msg = err.error().message().to_string();
    msg.contains("Output window was closed")
        || err
            .debug()
            .map(|d| d.to_string().contains("gst_xv_image_sink_handle_xevents"))
            .unwrap_or(false)
}

fn stop_tsp(mut tsp: Child) {
    let _ = tsp.kill();
    let _ = tsp.wait();
}

fn nudge_pipeline_playing(pipeline: &gst::Pipeline) {
    let ret = pipeline.set_state(gst::State::Playing);
    let cur = pipeline.current_state();
    if cur != gst::State::Playing {
        lastlog::event(
            "gst",
            &format!("nudge set_state(Playing) -> {ret:?}, current={cur:?}"),
        );
    }
}

/// After zap, try to adopt PCR clock from mpegtslivesrc (runs in background).
fn adopt_pcr_clock_when_ready(
    pipeline: gst::Pipeline,
    zap_ready: Arc<AtomicBool>,
    clock_src: Option<gst::Element>,
    clock_adopted: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    t0: Instant,
) {
    while !stop.load(Ordering::Relaxed) && !zap_ready.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(50));
    }
    if stop.load(Ordering::Relaxed) {
        return;
    }
    let Some(el) = clock_src else {
        return;
    };
    for _ in 0..120 {
        if stop.load(Ordering::Relaxed) || clock_adopted.load(Ordering::Relaxed) {
            return;
        }
        if adopt_pcr_clock(&pipeline, &el) {
            clock_adopted.store(true, Ordering::Relaxed);
            lastlog::event_elapsed("gst", t0, "PCR pipeline clock adopted");
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    lastlog::event("gst", "WARN: PCR clock not adopted — playback may drift/jerk");
}

impl GstWatch {
    /// Spawns `tsp` and builds the GStreamer pipeline (`tsp_args` ends at `-P zap <sid>`).
    pub fn spawn(
        tsp_bin: &Path,
        tsp_args: &[String],
        opts: GstOpts,
        t0: Instant,
    ) -> anyhow::Result<Self> {
        ensure_gstreamer()?;

        let pipeline = gst::Pipeline::new();
        let ingress = build_ts_ingress(&pipeline)?;
        let demux = pipeline
            .by_name("demux")
            .context("demux element")?;
        let appsrc = ingress.appsrc;

        let stats = GstStats::default();
        let stats_demux = stats.clone();
        let sync_diag = opts.sync_diag;
        let watch_audio = opts.audio;
        let linked = Arc::new(Mutex::new(PadLinkState::default()));
        let headless = opts.headless;
        let pcr_sync = ingress.clock_src.is_some();
        let clock_src = ingress.clock_src;
        let pipeline_weak = pipeline.downgrade();

        demux.connect_pad_added(move |_demux, src_pad| {
            let Some(pipeline) = pipeline_weak.upgrade() else {
                return;
            };
            let caps = match src_pad.current_caps() {
                Some(c) => c,
                None => return,
            };
            let structure = match caps.structure(0) {
                Some(s) => s,
                None => return,
            };
            let name = structure.name();
            lastlog::event("gst", &format!("demux pad {name}"));

            if name.starts_with("video/") {
                let mut guard = linked.lock().expect("pad link lock");
                if guard.video {
                    lastlog::event("gst", &format!("skip duplicate video pad ({name})"));
                    return;
                }
                match link_video(&pipeline, src_pad, headless, pcr_sync, &stats_demux, &name, t0) {
                    Ok(()) => {
                        guard.video = true;
                        lastlog::event_elapsed("gst", t0, &format!("video pads linked ({name})"));
                        flush_demux_ingress(&pipeline);
                    }
                    Err(e) => {
                        lastlog::event_elapsed("gst", t0, &format!("video link FAILED ({name}): {e:#}"));
                        eprintln!("[dvbhub] gst ERROR video link: {e:#}");
                    }
                }
            } else if name.starts_with("audio/") {
                if watch_audio == WatchAudioMode::None {
                    lastlog::event("gst", &format!("drop audio pad ({name}) — no audio mode"));
                    if let Err(e) = link_drop_pad(&pipeline, src_pad, t0, &name) {
                        lastlog::event("gst", &format!("drop audio pad failed: {e:#}"));
                    }
                    return;
                }
                let mut guard = linked.lock().expect("pad link lock");
                if guard.audio {
                    lastlog::event("gst", &format!("skip duplicate audio pad ({name})"));
                    return;
                }
                match link_audio(&pipeline, src_pad, headless, pcr_sync, &stats_demux, t0) {
                    Ok(()) => {
                        guard.audio = true;
                        lastlog::event_elapsed("gst", t0, &format!("audio pads linked ({name})"));
                    }
                    Err(e) => {
                        lastlog::event_elapsed("gst", t0, &format!("audio link FAILED ({name}): {e:#}"));
                        eprintln!("[dvbhub] gst ERROR audio link: {e:#}");
                    }
                }
            } else {
                lastlog::event("gst", &format!("drop other pad ({name})"));
                if let Err(e) = link_drop_pad(&pipeline, src_pad, t0, &name) {
                    lastlog::event("gst", &format!("drop pad failed: {e:#}"));
                }
            }
        });

        let mut tsp = Command::new(tsp_bin)
            .arg("--verbose")
            .args(tsp_args)
            .arg("-O")
            .arg("file")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawn tsp {}", tsp_bin.display()))?;
        lastlog::event_elapsed("gst", t0, &format!("tsp pid={}", tsp.id()));

        let tsp_stdout = tsp.stdout.take().context("tsp stdout")?;
        let tsp_stderr = tsp.stderr.take().context("tsp stderr")?;

        let stop = Arc::new(AtomicBool::new(false));
        let zap_ready = Arc::new(AtomicBool::new(false));
        let pcr_clock_adopted = Arc::new(AtomicBool::new(false));
        let mut handles = Vec::new();

        {
            let appsrc_feed = appsrc.clone();
            let stop_feed = Arc::clone(&stop);
            let ts_bytes = Arc::clone(&stats.ts_bytes);
            let pcr = stats.pcr.clone();
            let pcr_clock_adopted_feed = Arc::clone(&pcr_clock_adopted);
            let pipeline_feed = pipeline.clone();
            let clock_for_feed = clock_src.clone();
            handles.push(thread::spawn(move || {
                feed_appsrc(
                    tsp_stdout,
                    appsrc_feed,
                    stop_feed,
                    ts_bytes,
                    pcr,
                    pipeline_feed,
                    clock_for_feed,
                    pcr_clock_adopted_feed,
                );
            }));
        }

        {
            let stop_tsp = Arc::clone(&stop);
            let zap = Arc::clone(&zap_ready);
            handles.push(thread::spawn(move || tail_tsp_stderr(tsp_stderr, stop_tsp, zap, t0)));
        }

        {
            let stop_go = Arc::clone(&stop);
            let zap = Arc::clone(&zap_ready);
            let adopted = Arc::clone(&pcr_clock_adopted);
            let pipeline_go = pipeline.clone();
            let clock_go = clock_src.clone();
            handles.push(thread::spawn(move || {
                adopt_pcr_clock_when_ready(pipeline_go, zap, clock_go, adopted, stop_go, t0);
            }));
        }

        {
            let stop_stats = Arc::clone(&stop);
            let pipeline_stats = pipeline.clone();
            handles.push(thread::spawn(move || {
                poll_stats(pipeline_stats, stats, stop_stats, t0, sync_diag);
            }));
        }

        pipeline
            .set_state(gst::State::Playing)
            .context("pipeline PLAYING")?;
        pipeline.set_latency(gst::ClockTime::from_mseconds(875));
        let display = std::env::var("DISPLAY").unwrap_or_else(|_| "(unset)".into());
        let wayland = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "(unset)".into());
        lastlog::event("gst", &format!("DISPLAY={display} WAYLAND_DISPLAY={wayland}"));
        eprintln!("[dvbhub] gst video output DISPLAY={display} WAYLAND_DISPLAY={wayland}");
        lastlog::event_elapsed(
            "gst",
            t0,
            &format!(
                "pipeline PLAYING headless={headless} ts={} audio={:?}",
                ingress.mode_label, opts.audio
            ),
        );

        Ok(Self {
            pipeline,
            tsp,
            stop,
            handles,
        })
    }

    /// Runs the main loop until EOS, error, or `max_duration` elapses.
    pub fn wait(mut self, t0: Instant, max_duration: Option<Duration>) -> anyhow::Result<WatchEnd> {
        let bus = self.pipeline.bus().context("pipeline bus")?;
        let deadline = max_duration.map(|d| Instant::now() + d);
        let mut ok = true;
        let mut user_closed_video = false;

        loop {
            if let Some(dl) = deadline {
                if Instant::now() >= dl {
                    lastlog::event_elapsed(
                        "watch.test",
                        t0,
                        &format!(
                            "duration limit {:.0}s — stopping pipeline",
                            max_duration.unwrap().as_secs()
                        ),
                    );
                    break;
                }
            }

            match bus.timed_pop(gst::ClockTime::from_mseconds(200)) {
                None => continue,
                Some(msg) => match msg.view() {
                    gst::MessageView::Error(err) => {
                        if is_user_closed_video_error(&err) {
                            lastlog::event_elapsed("gst", t0, "user closed video window");
                            lastlog::event("watch", "user closed video window — stopping tsp/pipeline");
                            user_closed_video = true;
                            break;
                        }
                        let src = err
                            .src()
                            .map(|s| s.path_string())
                            .unwrap_or_else(|| "pipeline".into());
                        lastlog::event_elapsed(
                            "gst",
                            t0,
                            &format!(
                                "ERROR {src}: {} ({})",
                                err.error(),
                                err.debug().map(|d| d.to_string()).unwrap_or_default()
                            ),
                        );
                        eprintln!("[dvbhub] gst ERROR: {}", err.error());
                        ok = false;
                        break;
                    }
                    gst::MessageView::Warning(w) => {
                        lastlog::event(
                            "gst",
                            &format!(
                                "WARN: {} ({})",
                                w.error(),
                                w.debug().map(|d| d.to_string()).unwrap_or_default()
                            ),
                        );
                    }
                    gst::MessageView::Eos(_) => {
                        lastlog::event_elapsed("gst", t0, "EOS");
                        break;
                    }
                    gst::MessageView::AsyncDone(_) => {
                        lastlog::event("gst", "async done — pipeline preroll complete");
                    }
                    gst::MessageView::StateChanged(sc) => {
                        let src = sc
                            .src()
                            .map(|s| s.path_string())
                            .unwrap_or_else(|| "?".into());
                        lastlog::event(
                            "gst",
                            &format!(
                                "state {src}: {:?} → {:?}",
                                sc.old(),
                                sc.current()
                            ),
                        );
                    }
                    gst::MessageView::ClockProvide(cp) => {
                        if let Some(clock) = cp.clock() {
                            self.pipeline.use_clock(Some(&clock));
                            lastlog::event(
                                "gst",
                                &format!(
                                    "pipeline clock from {:?}",
                                    cp.src().map(|s| s.path_string())
                                ),
                            );
                        }
                    }
                    _ => {}
                },
            }
        }

        self.stop.store(true, Ordering::Relaxed);
        let _ = self.pipeline.set_state(gst::State::Null);
        stop_tsp(self.tsp);
        for h in self.handles {
            let _ = h.join();
        }
        let end = WatchEnd {
            ok,
            user_closed_video,
        };
        lastlog::event_elapsed(
            "gst",
            t0,
            &format!(
                "done ok={} user_closed={}",
                end.success(),
                end.user_closed_video
            ),
        );
        Ok(end)
    }
}

#[derive(Default)]
struct PadLinkState {
    video: bool,
    audio: bool,
}

/// After a late dynamic video link, flush tsdemux so h264parse sees the next SPS/PPS + IDR.
fn flush_demux_ingress(pipeline: &gst::Pipeline) {
    let Some(demux) = pipeline.by_name("demux") else {
        return;
    };
    let Some(sink) = demux.static_pad("sink") else {
        return;
    };
    let _ = sink.send_event(gst::event::FlushStart::new());
    let _ = sink.send_event(gst::event::FlushStop::new(true));
    lastlog::event("gst", "demux ingress flush (resync H264 after dynamic link)");
    eprintln!("[dvbhub] gst demux flushed — waiting for next keyframe");
}

/// Drain an unwanted tsdemux pad so demux does not stall with GST_FLOW_NOT_LINKED.
fn link_drop_pad(
    pipeline: &gst::Pipeline,
    src_pad: &gst::Pad,
    t0: Instant,
    label: &str,
) -> anyhow::Result<()> {
    lastlog::event("gst", &format!("drop pad link: {label} → queue ! fakesink"));
    let queue = make_leaky_queue()?;
    let sink = gst::ElementFactory::make("fakesink")
        .property("sync", false)
        .property("async", true)
        .build()?;
    pipeline.add_many([&queue, &sink])?;
    gst::Element::link_many([&queue, &sink])?;
    src_pad.link(&queue.static_pad("sink").context("drop queue sink")?)?;
    start_branch_playing(&[queue, sink], t0, "drop");
    Ok(())
}

fn link_video(
    pipeline: &gst::Pipeline,
    src_pad: &gst::Pad,
    headless: bool,
    _pcr_sync: bool,
    stats: &GstStats,
    caps_name: &str,
    t0: Instant,
) -> anyhow::Result<()> {
    let h265 = caps_name.contains("h265") || caps_name.contains("hevc");
    let parse_name = if h265 { "h265parse" } else { "h264parse" };
    let dec_name = if h265 { "avdec_h265" } else { "avdec_h264" };

    lastlog::event("gst", &format!("video link start: {caps_name} → {parse_name} → {dec_name}"));

    let queue = make_leaky_queue()?;
    let parse = gst::ElementFactory::make(parse_name)
        .property("config-interval", -1i32)
        .build()
        .with_context(|| parse_name.to_string())?;
    let dec = gst::ElementFactory::make(dec_name)
        .build()
        .with_context(|| dec_name.to_string())?;
    let vqueue = make_leaky_queue()?;
    let convert = gst::ElementFactory::make("videoconvert").build()?;
    let sink: gst::Element = if headless {
        gst::ElementFactory::make("identity")
            .property("sync", false)
            .build()?
    } else {
        make_live_video_sink()?
    };
    log_sink_choice("video", &sink);

    pipeline.add_many([&queue, &parse, &dec, &vqueue, &convert, &sink])?;
    gst::Element::link_many([&parse, &dec, &vqueue, &convert, &sink])?;

    let q_sink = queue.static_pad("sink").context("queue sink")?;
    src_pad.link(&q_sink)?;
    queue
        .static_pad("src")
        .context("queue src")?
        .link(&parse.static_pad("sink").context("parse sink")?)?;
    lastlog::event("gst", "video demux→queue→parse linked");

    if let Some(pad) = convert.static_pad("src") {
        let vb = Arc::clone(&stats.video_buffers);
        let lp = Arc::clone(&stats.last_video_pts_ns);
        let fp = Arc::clone(&stats.first_video_pts_ns);
        let pcr = stats.pcr.clone();
        let t0_probe = t0;
        pad.add_probe(gst::PadProbeType::BUFFER, move |_, info| {
            if let Some(buffer) = info.buffer() {
                let n = vb.fetch_add(1, Ordering::Relaxed) + 1;
                if n == 1 {
                    lastlog::event_elapsed("gst", t0_probe, "first decoded video frame");
                    eprintln!(
                        "[dvbhub] gst first video frame (+{:.1}s)",
                        t0_probe.elapsed().as_secs_f32()
                    );
                }
                if let Some(pts) = buffer.pts() {
                    let ns = pts.nseconds();
                    lp.store(ns, Ordering::Relaxed);
                    if fp.load(Ordering::Relaxed) == 0 {
                        fp.store(ns, Ordering::Relaxed);
                        pcr.mark_playback_start();
                    }
                }
            }
            gst::PadProbeReturn::Ok
        });
    }

    let branch = vec![
        queue.clone(),
        parse.clone(),
        dec.clone(),
        vqueue.clone(),
        convert.clone(),
        sink.clone(),
    ];
    start_branch_playing(&branch, t0, "video");
    Ok(())
}

fn link_audio(
    pipeline: &gst::Pipeline,
    src_pad: &gst::Pad,
    headless: bool,
    pcr_sync: bool,
    stats: &GstStats,
    t0: Instant,
) -> anyhow::Result<()> {
    lastlog::event("gst", "audio link start");
    let queue = gst::ElementFactory::make("queue").build()?;
    let decodebin = gst::ElementFactory::make("decodebin").build()?;
    let convert = gst::ElementFactory::make("audioconvert").build()?;
    let resample = gst::ElementFactory::make("audioresample").build()?;
    let sink: gst::Element = if headless {
        gst::ElementFactory::make("fakesink")
            .property("sync", false)
            .property("async", true)
            .build()?
    } else {
        make_audio_sink(pcr_sync)?
    };
    log_sink_choice("audio", &sink);

    pipeline.add_many([&queue, &decodebin, &convert, &resample, &sink])?;
    gst::Element::link_many([&convert, &resample, &sink])?;

    let q_sink = queue.static_pad("sink").context("queue sink")?;
    src_pad.link(&q_sink)?;
    queue
        .static_pad("src")
        .context("queue src")?
        .link(
            &decodebin
                .static_pad("sink")
                .context("decodebin sink")?,
        )?;

    let convert_clone = convert.clone();
    decodebin.connect_pad_added(move |_, pad| {
        if pad.direction() != gst::PadDirection::Src {
            return;
        }
        if let Some(sink_pad) = convert_clone.static_pad("sink") {
            if !sink_pad.is_linked() {
                let _ = pad.link(&sink_pad);
            }
        }
    });

    if let Some(pad) = resample.static_pad("src") {
        let ab = Arc::clone(&stats.audio_buffers);
        let t0_probe = t0;
        pad.add_probe(gst::PadProbeType::BUFFER, move |_, info| {
            if info.buffer().is_some() {
                let n = ab.fetch_add(1, Ordering::Relaxed) + 1;
                if n == 1 {
                    lastlog::event_elapsed("gst", t0_probe, "first decoded audio buffer");
                    eprintln!(
                        "[dvbhub] gst first audio buffer (+{:.1}s)",
                        t0_probe.elapsed().as_secs_f32()
                    );
                }
            }
            gst::PadProbeReturn::Ok
        });
    }

    let branch = vec![
        queue.clone(),
        decodebin.clone(),
        convert.clone(),
        resample.clone(),
        sink.clone(),
    ];
    start_branch_playing(&branch, t0, "audio");
    Ok(())
}

fn feed_appsrc(
    mut stdout: impl Read,
    appsrc: gst_app::AppSrc,
    stop: Arc<AtomicBool>,
    ts_bytes: Arc<AtomicU64>,
    pcr: PcrState,
    pipeline: gst::Pipeline,
    clock_src: Option<gst::Element>,
    clock_adopted: Arc<AtomicBool>,
) {
    let mut buf = [0u8; 188 * 64];
    let mut segment_sent = false;
    while !stop.load(Ordering::Relaxed) {
        match stdout.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if !segment_sent {
                    let mut segment = gst::FormattedSegment::<gst::format::Bytes>::new();
                    segment.set_start(gst::format::Bytes::ZERO);
                    appsrc.send_event(gst::event::Segment::new(&segment));
                    lastlog::event("gst", "appsrc BYTES segment before first TS push");
                    segment_sent = true;
                }
                ts_bytes.fetch_add(n as u64, Ordering::Relaxed);
                ts_pcr::scan_chunk_for_pcr(&buf[..n], &pcr);
                if let Some(ref clock_el) = clock_src {
                    try_adopt_pcr_clock_after_ts(&pipeline, clock_el, &pcr, &clock_adopted);
                }
                let buffer = gst::Buffer::from_slice(buf[..n].to_vec());
                match appsrc.push_buffer(buffer) {
                    Ok(gst::FlowSuccess::Ok) => {}
                    Ok(_) => break,
                    Err(e) => {
                        lastlog::event("gst", &format!("appsrc push failed: {e:?}"));
                        break;
                    }
                }
            }
            Err(e) => {
                lastlog::event("gst", &format!("tsp stdout read error: {e}"));
                break;
            }
        }
    }
    let _ = appsrc.end_of_stream();
}

fn tail_tsp_stderr(
    stderr: impl Read,
    stop: Arc<AtomicBool>,
    zap_ready: Arc<AtomicBool>,
    t0: Instant,
) {
    let reader = BufReader::new(stderr);
    for line in reader.lines().map_while(Result::ok) {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        lastlog::event("tsp", &line);
        if line.contains("zap: found service") {
            zap_ready.store(true, Ordering::Release);
        }
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

fn poll_stats(
    pipeline: gst::Pipeline,
    stats: GstStats,
    stop: Arc<AtomicBool>,
    t0: Instant,
    sync_diag: bool,
) {
    let mut last_pts = 0u64;
    let mut stall_ticks = 0u32;

    while !stop.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_secs(1));

        let pts_ns = stats.last_video_pts_ns.load(Ordering::Relaxed);
        let first_pts_ns = stats.first_video_pts_ns.load(Ordering::Relaxed);
        let pos_s = pipeline
            .query_position::<gst::ClockTime>()
            .map(|t| t.nseconds() as f64 / 1e9)
            .unwrap_or_else(|| pts_ns as f64 / 1e9);
        let vbuf = stats.video_buffers.load(Ordering::Relaxed);
        let abuf = stats.audio_buffers.load(Ordering::Relaxed);
        let ts = stats.ts_bytes.load(Ordering::Relaxed);

        if vbuf == 0 && ts > 500_000 {
            let cur = pipeline.current_state();
            if cur != gst::State::Playing {
                nudge_pipeline_playing(&pipeline);
            }
        }

        let line = format!("time={pos_s:.2}s video_bufs={vbuf} audio_bufs={abuf} ts_bytes={ts}");
        lastlog::event("gst.stats", &line);
        eprintln!("[dvbhub] gst {line}");

        if sync_diag || pts_ns > 0 {
            log_sync_diag(&stats, pts_ns, first_pts_ns, pos_s, sync_diag);
        }

        if pts_ns > 0 {
            if pts_ns == last_pts {
                stall_ticks += 1;
                if stall_ticks == 3 {
                    lastlog::event_elapsed(
                        "watch",
                        t0,
                        &format!("STALL: video pts stuck at {:.2}s", pts_ns as f64 / 1e9),
                    );
                    eprintln!(
                        "[dvbhub] WARN: playback stalled at {:.1}s",
                        pts_ns as f64 / 1e9
                    );
                }
            } else {
                stall_ticks = 0;
            }
            last_pts = pts_ns;
        }
    }
}

/// Compares broadcaster PCR, demux video PTS, pipeline position, and wall clock.
fn log_sync_diag(
    stats: &GstStats,
    pts_ns: u64,
    first_pts_ns: u64,
    pipe_pos_s: f64,
    verbose: bool,
) {
    let pcr_elapsed = stats.pcr.pcr_elapsed_secs();
    let wall = stats.pcr.wall_elapsed_secs();
    let pcr_count = stats.pcr.pcr_count.load(Ordering::Relaxed);

    if pcr_elapsed.is_none() && pts_ns == 0 {
        return;
    }

    let video_pts_s = pts_ns as f64 / 1e9;
    let video_rel_s = if first_pts_ns > 0 {
        (pts_ns.saturating_sub(first_pts_ns)) as f64 / 1e9
    } else {
        0.0
    };

    let pcr_s = pcr_elapsed.unwrap_or(0.0);
    let wall_s = wall.unwrap_or(0.0);
    let drift_wall_pcr = wall_s - pcr_s;
    let drift_pts_wall = video_rel_s - wall_s;

    let line = format!(
        "pcr={pcr_s:.2}s video_pts={video_pts_s:.2}s video_rel={video_rel_s:.2}s \
         pipe_pos={pipe_pos_s:.2}s wall={wall_s:.2}s \
         drift_pcr_wall={drift_wall_pcr:+.2}s drift_pts_wall={drift_pts_wall:+.2}s \
         pcr_pkts={pcr_count} (drift_pcr_wall≈0 → TS real-time; |pts-wall|<0.5 → decode ok)"
    );
    lastlog::event("gst.sync", &line);
    if verbose || wall_s > 0.0 {
        eprintln!("[dvbhub] sync {line}");
    }

    if wall_s > 3.0 && drift_pts_wall.abs() > 2.0 {
        lastlog::event(
            "gst.sync",
            &format!("WARN large pts/wall drift {drift_pts_wall:+.2}s — sync may be wrong"),
        );
    }
}

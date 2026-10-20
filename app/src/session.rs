//! Web session manager: runs `tsp` jobs for the web UI and fans out progress.
//!
//! Two independent job slots:
//!
//! * **tuner** — one of monitor / capture / scan / play at a time (they all
//!   need the tuner). Starting a new tuner job stops the current one.
//! * **modulator** — DTU-315 playout. Runs alongside a tuner job, so a
//!   DTU-315 → TBS5530 loopback (play out + capture) works from the browser.
//!
//! Capture and scan reuse the CLI/TUI engines (`sat_capture`, `terr_capture`,
//! `cable_capture`, `channel_scan`), so file naming, `_services.json` sidecars
//! and signal statistics are identical everywhere. Every event is pushed to
//! WebSocket subscribers and appended to the log file.

use crate::analyze::ServiceInfo;
use crate::cable::CableMux;
use crate::channel_scan::{self, ScanEvent, ScanOptions, ScanTarget, TargetResult};
use crate::channels::ChannelDb;
use crate::config::{AppConfig, TsduckPaths};
use crate::devices::DvbDevice;
use crate::pipeline::{CaptureParams, PlayParams};
use crate::progress::CaptureEvent;
use crate::satellites::{SatTransponder, Satellite};
use crate::signal::{parse_dvbstatus_line, parse_signal_line};
use crate::terrestrial::TerrestrialMux;
use anyhow::Context;
use serde::Serialize;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, Mutex, RwLock};

pub use crate::signal::SignalStatus;

/// Progress of the running capture job.
#[derive(Debug, Clone, Serialize, Default)]
pub struct CaptureProgress {
    pub label: String,
    /// 1-based index of the current target.
    pub index: usize,
    pub total: usize,
    pub current: String,
    pub remaining_s: Option<u32>,
    pub files: Vec<String>,
}

/// Progress of the running scan job.
#[derive(Debug, Clone, Serialize, Default)]
pub struct ScanProgress {
    pub index: usize,
    pub total: usize,
    pub current: String,
    pub locked: usize,
    pub found: usize,
}

/// Live playback info.
#[derive(Debug, Clone, Serialize)]
pub struct PlayInfo {
    pub hls_url: String,
    pub service_id: u64,
    pub channel_id: Option<u64>,
    pub name: String,
    pub ready: bool,
    /// Seconds from start until the first HLS manifest appeared.
    pub ready_after_s: Option<f64>,
}

/// Snapshot pushed to the UI on every change.
#[derive(Debug, Clone, Serialize, Default)]
pub struct SessionStatus {
    pub monitoring: bool,
    pub capturing: bool,
    pub scanning: bool,
    pub playing: bool,
    pub playout: bool,
    pub device: Option<String>,
    pub signal: SignalStatus,
    pub services: Vec<ServiceInfo>,
    pub capture: Option<CaptureProgress>,
    pub scan: Option<ScanProgress>,
    pub play: Option<PlayInfo>,
    pub playout_file: Option<String>,
    pub last_error: Option<String>,
}

/// Events pushed to WebSocket subscribers.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsEvent {
    Status { status: SessionStatus },
    Signal { signal: SignalStatus },
    Services { services: Vec<ServiceInfo> },
    Devices { devices: Vec<DvbDevice>, warnings: Vec<String> },
    ScanTarget { total: usize, kept: usize, result: TargetResult },
    ScanDone { locked: usize, total: usize, found: usize, added: usize, cancelled: bool },
    CaptureDone { files: Vec<String>, ok: usize, failed: usize, cancelled: bool },
    PlayReady { hls_url: String, service_id: u64 },
    Error { message: String },
    Log { line: String },
}

/// A resolved capture job.
pub enum CaptureJob {
    /// One or more TPs of a satellite (catalogue entry or manual definition).
    Satellite { satellite: Satellite, device: String, seconds: u32 },
    Terrestrial { mux: TerrestrialMux, device: String, seconds: u32 },
    Cable { mux: CableMux, device: String, seconds: u32 },
}

/// A resolved channel scan job.
pub struct ScanJob {
    pub device: String,
    pub targets: Vec<ScanTarget>,
    pub opts: ScanOptions,
    pub save: bool,
    pub replace: bool,
}

/// A resolved playback (watch) job.
pub struct PlayJob {
    pub params: CaptureParams,
    pub service_id: u64,
    pub channel_id: Option<u64>,
    pub name: String,
}

struct Job {
    id: u64,
    kind: &'static str,
    cancel: Arc<AtomicBool>,
    child: Option<Child>,
    task: Option<tokio::task::JoinHandle<()>>,
}

#[derive(Default)]
struct Inner {
    tuner: Option<Job>,
    modulator: Option<Job>,
}

/// Shared session manager.
pub struct SessionManager {
    paths: Arc<TsduckPaths>,
    config: Arc<RwLock<AppConfig>>,
    channels: Arc<RwLock<ChannelDb>>,
    inner: Mutex<Inner>,
    status: RwLock<SessionStatus>,
    bus: broadcast::Sender<WsEvent>,
    next_id: AtomicU64,
}

impl SessionManager {
    /// Creates the manager; also starts the file logger.
    pub fn new(
        paths: Arc<TsduckPaths>,
        config: Arc<RwLock<AppConfig>>,
        channels: Arc<RwLock<ChannelDb>>,
    ) -> Arc<Self> {
        let (tx, _) = broadcast::channel(1024);
        tokio::spawn(file_logger(tx.subscribe(), crate::config::log_file_path()));
        Arc::new(Self {
            paths,
            config,
            channels,
            inner: Mutex::new(Inner::default()),
            status: RwLock::new(SessionStatus::default()),
            bus: tx,
            next_id: AtomicU64::new(1),
        })
    }

    /// New receiver for a WebSocket client.
    pub fn subscribe(&self) -> broadcast::Receiver<WsEvent> {
        self.bus.subscribe()
    }

    /// Sender for non-async producers (device monitor thread).
    pub fn sender(&self) -> broadcast::Sender<WsEvent> {
        self.bus.clone()
    }

    pub async fn status(&self) -> SessionStatus {
        self.status.read().await.clone()
    }

    /// Logs a user-action marker (`>>> …`).
    fn action(&self, msg: impl Into<String>) {
        let line = format!(">>> {}", msg.into());
        crate::lastlog::event("web", &line);
        let _ = self.bus.send(WsEvent::Log { line });
    }

    async fn set_status(&self, f: impl FnOnce(&mut SessionStatus)) {
        let st = {
            let mut st = self.status.write().await;
            f(&mut st);
            st.clone()
        };
        let _ = self.bus.send(WsEvent::Status { status: st });
    }

    async fn push_signal(&self, signal: SignalStatus) {
        self.status.write().await.signal = signal.clone();
        let _ = self.bus.send(WsEvent::Signal { signal });
    }

    async fn push_services(&self, services: Vec<ServiceInfo>) {
        self.status.write().await.services = services.clone();
        let _ = self.bus.send(WsEvent::Services { services });
    }

    async fn error(&self, message: String) {
        crate::lastlog::event("web.error", &message);
        let _ = self.bus.send(WsEvent::Error { message: message.clone() });
        self.set_status(|st| st.last_error = Some(message)).await;
    }

    /// Updates the capture directory, persists the config and creates the dir.
    pub async fn set_capture_dir(&self, dir: std::path::PathBuf) -> anyhow::Result<AppConfig> {
        tokio::fs::create_dir_all(&dir)
            .await
            .with_context(|| format!("cannot create capture dir {}", dir.display()))?;
        let cfg = {
            let mut c = self.config.write().await;
            c.capture_dir = dir;
            c.clone()
        };
        let to_save = cfg.clone();
        tokio::task::spawn_blocking(move || to_save.save())
            .await
            .map_err(|e| anyhow::anyhow!("config save task failed: {e}"))?
            .context("failed to persist config")?;
        self.action(format!("SETTINGS capture_dir = {}", cfg.capture_dir.display()));
        Ok(cfg)
    }

    /// Sets and persists the browser playback quality.
    pub async fn set_web_quality(&self, quality: &str) -> anyhow::Result<()> {
        anyhow::ensure!(
            crate::config::WEB_QUALITIES.contains(&quality),
            "web_quality must be one of {:?}",
            crate::config::WEB_QUALITIES
        );
        let cfg = {
            let mut c = self.config.write().await;
            c.web_quality = quality.to_string();
            c.clone()
        };
        tokio::task::spawn_blocking(move || cfg.save())
            .await
            .map_err(|e| anyhow::anyhow!("config save task failed: {e}"))?
            .context("failed to persist config")?;
        self.action(format!("SETTINGS web_quality = {quality}"));
        Ok(())
    }

    // ---- job slots -----------------------------------------------------

    fn new_job(&self, kind: &'static str) -> Job {
        Job {
            id: self.next_id.fetch_add(1, Ordering::SeqCst),
            kind,
            cancel: Arc::new(AtomicBool::new(false)),
            child: None,
            task: None,
        }
    }

    /// Stops the current tuner job (monitor / capture / scan / play).
    pub async fn stop_tuner(&self) {
        let job = self.inner.lock().await.tuner.take();
        if let Some(job) = job {
            self.action(format!("{} stop", job.kind.to_uppercase()));
            stop_job(job).await;
        }
        self.set_status(|st| {
            st.monitoring = false;
            st.capturing = false;
            st.scanning = false;
            st.playing = false;
            st.play = None;
        })
        .await;
        clean_hls_tree(&hls_root()).await;
    }

    /// Stops the DTU-315 playout.
    pub async fn stop_playout(&self) {
        let job = self.inner.lock().await.modulator.take();
        if let Some(job) = job {
            self.action("PLAYOUT stop");
            stop_job(job).await;
        }
        self.set_status(|st| {
            st.playout = false;
            st.playout_file = None;
        })
        .await;
    }

    /// Clears the tuner slot if `id` is still the active job (natural end).
    async fn finish_tuner_job(&self, id: u64) -> bool {
        let mut inner = self.inner.lock().await;
        if inner.tuner.as_ref().is_some_and(|j| j.id == id) {
            inner.tuner = None;
            true
        } else {
            false
        }
    }

    // ---- monitor -------------------------------------------------------

    /// Tunes and shows live signal + services (`tsp … -P analyze --interval 3 -O drop`).
    pub async fn start_monitor(self: &Arc<Self>, params: CaptureParams) -> anyhow::Result<()> {
        self.stop_tuner().await;
        let mut args = params.to_tsp_args();
        if let Some(i) = args.iter().position(|a| a == "-O") {
            args.truncate(i);
        }
        args.extend(
            ["-P", "analyze", "--json", "--interval", "3", "-O", "drop"]
                .iter()
                .map(|s| s.to_string()),
        );
        let device = params.device_name.clone().unwrap_or_default();
        self.action(format!(
            "MONITOR start {device} {} MHz",
            params.frequency_hz / 1_000_000
        ));
        let mut child = spawn_tsp(&self.paths, &args)?;
        let (out, err) = (child.stdout.take(), child.stderr.take());
        let mgr = Arc::clone(self);
        let mut job = self.new_job("monitor");
        job.task = Some(tokio::spawn(async move {
            if let (Some(o), Some(e)) = (out, err) {
                read_monitor_output(mgr, o, e).await;
            }
        }));
        job.child = Some(child);
        self.inner.lock().await.tuner = Some(job);
        self.set_status(|st| {
            st.monitoring = true;
            st.device = Some(device);
            st.signal = SignalStatus::default();
            st.services.clear();
            st.last_error = None;
        })
        .await;
        Ok(())
    }

    // ---- capture -------------------------------------------------------

    /// Starts a capture job (one TP, a whole satellite, a terrestrial or cable mux).
    pub async fn start_capture(self: &Arc<Self>, job_spec: CaptureJob) -> anyhow::Result<()> {
        self.stop_tuner().await;
        let capture_dir = self.config.read().await.capture_dir.clone();
        let (label, device, total, targets): (String, String, usize, Vec<String>) = match &job_spec
        {
            CaptureJob::Satellite { satellite, device, .. } => (
                satellite.name.clone(),
                device.clone(),
                satellite.transponders.len(),
                satellite
                    .transponders
                    .iter()
                    .map(|t| format!("{} {}", t.frequency_mhz, pol_initial(&t.polarity)))
                    .collect(),
            ),
            CaptureJob::Terrestrial { mux, device, .. } => {
                (mux.label.clone(), device.clone(), 1, vec![mux.label.clone()])
            }
            CaptureJob::Cable { mux, device, .. } => {
                (mux.label.clone(), device.clone(), 1, vec![mux.label.clone()])
            }
        };
        anyhow::ensure!(total > 0, "nothing to capture");
        self.action(format!("CAPTURE start {label} ({total} target(s)) on {device}"));

        let mut job = self.new_job("capture");
        let id = job.id;
        let cancel = Arc::clone(&job.cancel);
        let mgr = Arc::clone(self);
        let paths = Arc::clone(&self.paths);
        let handle = tokio::runtime::Handle::current();
        job.task = Some(tokio::task::spawn_blocking(move || {
            let (tx, rx) = std::sync::mpsc::channel::<CaptureEvent>();
            let fwd_mgr = Arc::clone(&mgr);
            let fwd_handle = handle.clone();
            let forwarder = std::thread::spawn(move || {
                for ev in rx {
                    fwd_handle.block_on(fwd_mgr.on_capture_event(ev));
                }
            });
            let mut files = Vec::new();
            let (mut ok, mut failed) = (0usize, 0usize);
            run_capture_job(
                &paths,
                &job_spec,
                &capture_dir,
                &cancel,
                &tx,
                |index, current| {
                    handle.block_on(mgr.set_status(|st| {
                        if let Some(c) = st.capture.as_mut() {
                            c.index = index;
                            c.current = current.to_string();
                            c.remaining_s = None;
                        }
                    }));
                },
                |result| match result {
                    Ok((locked, ts, sidecar)) => {
                        if locked {
                            ok += 1;
                        } else {
                            failed += 1;
                        }
                        let line = if sidecar.is_file() {
                            format!("saved {} (+ sidecar)", ts.display())
                        } else {
                            format!("saved {}", ts.display())
                        };
                        let _ = mgr.bus.send(WsEvent::Log { line });
                        files.push(ts.to_string_lossy().into_owned());
                        let f = files.clone();
                        handle.block_on(mgr.set_status(|st| {
                            if let Some(c) = st.capture.as_mut() {
                                c.files = f;
                            }
                        }));
                    }
                    Err(e) => {
                        failed += 1;
                        handle.block_on(mgr.error(format!("capture failed: {e:#}")));
                    }
                },
            );
            drop(tx);
            let _ = forwarder.join();
            let cancelled = cancel.load(Ordering::SeqCst);
            handle.block_on(async {
                if mgr.finish_tuner_job(id).await {
                    mgr.set_status(|st| st.capturing = false).await;
                }
                mgr.action(format!(
                    "CAPTURE {}: {ok} ok, {failed} failed",
                    if cancelled { "stopped" } else { "done" }
                ));
                let _ = mgr.bus.send(WsEvent::CaptureDone {
                    files,
                    ok,
                    failed,
                    cancelled,
                });
            });
        }));
        self.inner.lock().await.tuner = Some(job);
        self.set_status(|st| {
            st.capturing = true;
            st.device = Some(device);
            st.signal = SignalStatus::default();
            st.services.clear();
            st.capture = Some(CaptureProgress {
                label,
                index: 0,
                total,
                current: targets.first().cloned().unwrap_or_default(),
                remaining_s: None,
                files: Vec::new(),
            });
            st.last_error = None;
        })
        .await;
        Ok(())
    }

    async fn on_capture_event(&self, ev: CaptureEvent) {
        match ev {
            CaptureEvent::Signal(sig) => self.push_signal(sig).await,
            CaptureEvent::Services(lines) => {
                let _ = self.bus.send(WsEvent::Log {
                    line: format!("services: {}", lines.join(" · ")),
                });
            }
            CaptureEvent::RecordingRemaining(s) => {
                self.set_status(|st| {
                    if let Some(c) = st.capture.as_mut() {
                        c.remaining_s = Some(s);
                    }
                })
                .await
            }
            CaptureEvent::Command(line) | CaptureEvent::Status(line) | CaptureEvent::Log(line) => {
                let _ = self.bus.send(WsEvent::Log { line });
            }
        }
    }

    // ---- scan ----------------------------------------------------------

    /// Starts a channel scan; found channels are optionally saved.
    pub async fn start_scan(self: &Arc<Self>, spec: ScanJob) -> anyhow::Result<()> {
        anyhow::ensure!(!spec.targets.is_empty(), "nothing to scan");
        self.stop_tuner().await;
        let total = spec.targets.len();
        self.action(format!("SCAN start {total} target(s) on {}", spec.device));
        let mut job = self.new_job("scan");
        let id = job.id;
        let cancel = Arc::clone(&job.cancel);
        let mgr = Arc::clone(self);
        let paths = Arc::clone(&self.paths);
        let handle = tokio::runtime::Handle::current();
        let device = spec.device.clone();
        job.task = Some(tokio::task::spawn_blocking(move || {
            let (mut locked, mut found) = (0usize, 0usize);
            let results = channel_scan::scan(&paths, &spec.targets, &spec.opts, &cancel, |ev| {
                match ev {
                    ScanEvent::TargetStart { index, total, label } => {
                        handle.block_on(mgr.set_status(|st| {
                            st.scan = Some(ScanProgress {
                                index: index + 1,
                                total,
                                current: label,
                                locked,
                                found,
                            });
                        }));
                    }
                    ScanEvent::TargetDone { total, result } => {
                        if result.locked {
                            locked += 1;
                            found += result.wanted_count(&spec.opts);
                            handle.block_on(mgr.push_signal(SignalStatus {
                                locked: true,
                                snr_db: result.snr_db,
                                strength_db: result.strength_db,
                                ..Default::default()
                            }));
                        }
                        let kept = result.wanted_count(&spec.opts);
                        let _ = mgr.bus.send(WsEvent::ScanTarget { total, kept, result });
                    }
                }
            });
            let channels = channel_scan::channels_from_results(&results, &spec.targets, &spec.opts);
            let cancelled = cancel.load(Ordering::SeqCst);
            handle.block_on(async {
                let mut added = 0;
                if spec.save && !channels.is_empty() {
                    let db = {
                        let mut db = mgr.channels.write().await;
                        if spec.replace {
                            let first = &spec.targets[0];
                            db.channels.retain(|c| {
                                !(c.delivery == first.delivery && c.satellite == first.satellite)
                            });
                        }
                        let base = db.channels.iter().map(|c| c.scan_index).max().unwrap_or(0);
                        let batch = channels
                            .iter()
                            .cloned()
                            .enumerate()
                            .map(|(i, mut c)| {
                                c.scan_index = base + 1 + i as u32;
                                c
                            })
                            .collect();
                        added = db.add_batch(batch);
                        db.clone()
                    };
                    if let Err(e) = tokio::task::spawn_blocking(move || db.save()).await {
                        mgr.error(format!("saving channels failed: {e}")).await;
                    }
                }
                if mgr.finish_tuner_job(id).await {
                    mgr.set_status(|st| st.scanning = false).await;
                }
                mgr.action(format!(
                    "SCAN {}: {locked}/{} locked, {} channel(s), {added} added",
                    if cancelled { "stopped" } else { "done" },
                    results.len(),
                    channels.len()
                ));
                let _ = mgr.bus.send(WsEvent::ScanDone {
                    locked,
                    total: results.len(),
                    found: channels.len(),
                    added,
                    cancelled,
                });
            });
        }));
        self.inner.lock().await.tuner = Some(job);
        self.set_status(|st| {
            st.scanning = true;
            st.device = Some(device);
            st.signal = SignalStatus::default();
            st.scan = Some(ScanProgress {
                total,
                ..Default::default()
            });
            st.last_error = None;
        })
        .await;
        Ok(())
    }

    // ---- play (watch in the browser) ------------------------------------

    /// Starts live playback: `tsp -P zap <sid> -O fork ffmpeg … HLS`.
    pub async fn start_play(self: &Arc<Self>, spec: PlayJob) -> anyhow::Result<String> {
        self.stop_tuner().await;
        let id = self.next_id.load(Ordering::SeqCst);
        let dir = hls_root().join(format!("s{}-{id}", spec.service_id));
        tokio::fs::create_dir_all(&dir)
            .await
            .with_context(|| format!("cannot create HLS dir {}", dir.display()))?;
        let m3u8 = dir.join("stream.m3u8");
        let mut args = spec.params.to_tsp_args();
        if let Some(i) = args.iter().position(|a| a == "-O") {
            args.truncate(i);
        }
        // Video is re-encoded for the browser: broadcast video is often
        // interlaced, open-GOP H.264 (no IDR frames; Chrome's decoder rejects the
        // leading pictures) or MPEG-2 (not playable in browsers at all). A cheap
        // 720p x264 encode (~0.5 core) with an IDR every second makes every 1 s
        // segment start cleanly, so playback begins within a few seconds.
        // Audio is AAC (browsers cannot play MPEG-1/2 audio). Only the first
        // video and audio streams are mapped; teletext/data would break the muxer.
        let quality = self.config.read().await.web_quality.clone();
        let video = web_video_args(&quality);
        let ffmpeg = format!(
            "ffmpeg -hide_banner -loglevel error -fflags +genpts+discardcorrupt+nobuffer \
             -analyzeduration 1M -probesize 1M -i pipe:0 -map 0:v:0 -map 0:a:0? \
             {video} -tune zerolatency -g 25 -keyint_min 25 -sc_threshold 0 \
             -c:a aac -b:a 128k -ac 2 -f hls -hls_time 1 -hls_list_size 6 \
             -hls_flags delete_segments+omit_endlist+independent_segments \
             -hls_segment_type fmp4 -hls_fmp4_init_filename init.mp4 {}",
            m3u8.display()
        );
        args.extend(["-P".into(), "zap".into(), spec.service_id.to_string(), "-O".into(), "fork".into(), ffmpeg]);
        let url = format!("/hls/s{}-{id}/stream.m3u8", spec.service_id);
        self.action(format!(
            "PLAY start '{}' sid={} quality={quality} on {}",
            spec.name,
            spec.service_id,
            spec.params.device_name.clone().unwrap_or_default()
        ));
        let mut child = spawn_tsp(&self.paths, &args)?;
        let mut job = self.new_job("play");
        let job_id = job.id;
        let logger = spawn_child_logger(self, &mut child);
        let mgr = Arc::clone(self);
        let (sid, url_ready, m3u8_wait) = (spec.service_id, url.clone(), m3u8.clone());
        let started = Instant::now();
        job.task = Some(tokio::spawn(async move {
            // Announce once the manifest lists at least one segment.
            let deadline = started + Duration::from_secs(45);
            let mut ready = false;
            while Instant::now() < deadline {
                if !mgr.is_active(job_id).await {
                    break;
                }
                if let Ok(text) = tokio::fs::read_to_string(&m3u8_wait).await {
                    if text.contains("#EXTINF") {
                        ready = true;
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            if ready {
                let secs = started.elapsed().as_secs_f64();
                mgr.action(format!("PLAY ready after {secs:.1} s"));
                mgr.set_status(|st| {
                    if let Some(p) = st.play.as_mut() {
                        p.ready = true;
                        p.ready_after_s = Some(secs);
                    }
                })
                .await;
                let _ = mgr.bus.send(WsEvent::PlayReady {
                    hls_url: url_ready,
                    service_id: sid,
                });
            } else if mgr.is_active(job_id).await {
                mgr.error("no video after 45 s (no lock, scrambled or off-air service?)".into())
                    .await;
            }
            if let Some(l) = logger {
                let _ = l.await;
            }
            // tsp ended (or job replaced): clear the slot if it is still ours.
            if mgr.finish_tuner_job(job_id).await {
                mgr.set_status(|st| {
                    st.playing = false;
                    st.play = None;
                })
                .await;
            }
        }));
        job.child = Some(child);
        self.inner.lock().await.tuner = Some(job);
        self.set_status(|st| {
            st.playing = true;
            st.device = spec.params.device_name.clone();
            st.signal = SignalStatus::default();
            st.play = Some(PlayInfo {
                hls_url: url.clone(),
                service_id: spec.service_id,
                channel_id: spec.channel_id,
                name: spec.name.clone(),
                ready: false,
                ready_after_s: None,
            });
            st.last_error = None;
        })
        .await;
        Ok(url)
    }

    async fn is_active(&self, id: u64) -> bool {
        self.inner.lock().await.tuner.as_ref().is_some_and(|j| j.id == id)
    }

    // ---- playout (DTU-315) -------------------------------------------------

    /// Starts a playout; independent of the tuner slot.
    pub async fn start_playout(self: &Arc<Self>, params: PlayParams) -> anyhow::Result<()> {
        self.stop_playout().await;
        let probe_paths = Arc::clone(&self.paths);
        let probe_params = params.clone();
        let (args, note) = tokio::task::spawn_blocking(move || {
            crate::pipeline::playout_args(&probe_paths, &probe_params)
        })
        .await??;
        self.action(format!("PLAYOUT rate: {note}"));
        self.action(format!(
            "PLAYOUT start {} @ {} MHz {}{}",
            params.input,
            params.frequency_hz / 1_000_000,
            params.modulation,
            if params.infinite { " (loop)" } else { "" }
        ));
        let mut child = spawn_tsp(&self.paths, &args)?;
        let logger = spawn_child_logger(self, &mut child);
        let mut job = self.new_job("playout");
        let id = job.id;
        let mgr = Arc::clone(self);
        job.task = Some(tokio::spawn(async move {
            if let Some(l) = logger {
                let _ = l.await;
            }
            // stdout/stderr closed: tsp finished (end of file) or was stopped.
            let mine = {
                let mut inner = mgr.inner.lock().await;
                if inner.modulator.as_ref().is_some_and(|j| j.id == id) {
                    inner.modulator.take()
                } else {
                    None
                }
            };
            if let Some(job) = mine {
                stop_job(job).await;
                mgr.action("PLAYOUT finished");
                mgr.set_status(|st| {
                    st.playout = false;
                    st.playout_file = None;
                })
                .await;
            }
        }));
        job.child = Some(child);
        self.inner.lock().await.modulator = Some(job);
        let file = params.input.clone();
        self.set_status(|st| {
            st.playout = true;
            st.playout_file = Some(file);
            st.last_error = None;
        })
        .await;
        Ok(())
    }
}

/// Runs a capture job synchronously (blocking thread). `on_target` is called
/// before each target, `on_done` after it with `(locked, ts, sidecar)`.
fn run_capture_job(
    paths: &TsduckPaths,
    job: &CaptureJob,
    capture_dir: &std::path::Path,
    cancel: &Arc<AtomicBool>,
    events: &std::sync::mpsc::Sender<CaptureEvent>,
    mut on_target: impl FnMut(usize, &str),
    mut on_done: impl FnMut(anyhow::Result<(bool, std::path::PathBuf, std::path::PathBuf)>),
) {
    const SIGNAL_TIMEOUT_S: u32 = 10;
    match job {
        CaptureJob::Satellite { satellite, device, seconds } => {
            let root = capture_dir.join(crate::sat_capture::satellite_slug(&satellite.name));
            for (i, tp) in satellite.transponders.iter().enumerate() {
                if cancel.load(Ordering::SeqCst) {
                    break;
                }
                on_target(i + 1, &tp_label(tp));
                let r = crate::sat_capture::capture_transponder(
                    paths,
                    satellite,
                    device,
                    tp,
                    *seconds,
                    SIGNAL_TIMEOUT_S,
                    &root,
                    true,
                    Some(events.clone()),
                    Some(Arc::clone(cancel)),
                )
                .map(|o| (o.run.ever_locked, o.ts_path, o.services_path));
                on_done(r);
            }
        }
        CaptureJob::Terrestrial { mux, device, seconds } => {
            on_target(1, &mux.label);
            let r = crate::terr_capture::capture_mux(
                paths,
                device,
                mux,
                *seconds,
                SIGNAL_TIMEOUT_S,
                &capture_dir.join("terrestrial"),
                true,
                Some(events.clone()),
                Some(Arc::clone(cancel)),
            )
            .map(|o| (o.run.ever_locked, o.ts_path, o.services_path));
            on_done(r);
        }
        CaptureJob::Cable { mux, device, seconds } => {
            on_target(1, &mux.label);
            let r = crate::cable_capture::capture_mux(
                paths,
                device,
                mux,
                *seconds,
                SIGNAL_TIMEOUT_S,
                &capture_dir.join("cable"),
                true,
                Some(events.clone()),
                Some(Arc::clone(cancel)),
            )
            .map(|o| (o.run.ever_locked, o.ts_path, o.services_path));
            on_done(r);
        }
    }
}

fn tp_label(tp: &SatTransponder) -> String {
    format!(
        "{} {} {} {}",
        tp.frequency_mhz,
        pol_initial(&tp.polarity),
        tp.symbol_rate_ksps,
        tp.delivery_system
    )
}

fn pol_initial(p: &str) -> char {
    p.chars().next().unwrap_or('?').to_ascii_uppercase()
}

/// ffmpeg video options for a browser playback quality: "720p" (default) or
/// "source" (native resolution, e.g. 3840x2160 for UHD channels; a 4K50 HEVC
/// source encodes ~2x faster than real time with x264 superfast). Interlaced
/// sources are deinterlaced (yadif only touches interlaced frames).
fn web_video_args(quality: &str) -> String {
    let (filter, preset, rate, max, buf) = match quality {
        // The comma inside min() is escaped for ffmpeg; the single quotes keep
        // the backslash through the shell that tsp's fork output uses.
        "source" => ("yadif=0:-1:1", "superfast", "16M", "20M", "32M"),
        _ => (r"yadif=0:-1:1,scale=-2:min(720\,ih)", "veryfast", "3M", "3500k", "6M"),
    };
    // 8-bit output: 10-bit sources (UHD HEVC Main 10) would otherwise give High 10
    // H.264, which browsers do not decode.
    format!(
        "-vf '{filter}' -pix_fmt yuv420p -c:v libx264 -preset {preset} -b:v {rate} -maxrate {max} -bufsize {buf}"
    )
}

/// Stops a job: cancel flag, SIGTERM to its child, then wait for the task.
async fn stop_job(mut job: Job) {
    job.cancel.store(true, Ordering::SeqCst);
    if let Some(mut c) = job.child.take() {
        graceful_stop(&mut c).await;
    }
    if let Some(t) = job.task.take() {
        // Blocking capture/scan jobs notice the flag within ~0.5 s and kill tsp.
        let _ = tokio::time::timeout(Duration::from_secs(15), t).await;
    }
}

/// HLS working directory on tmpfs (`$XDG_RUNTIME_DIR/dvbhub-hls`, else `/tmp`).
pub fn hls_root() -> std::path::PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_dir())
        .unwrap_or_else(std::env::temp_dir)
        .join("dvbhub-hls")
}

/// Removes all per-session HLS directories.
async fn clean_hls_tree(root: &std::path::Path) {
    if let Ok(mut entries) = tokio::fs::read_dir(root).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let _ = tokio::fs::remove_dir_all(entry.path()).await;
        }
    }
}

/// Streams a child's stdout/stderr to the log bus (signal lines update status).
fn spawn_child_logger(
    mgr: &Arc<SessionManager>,
    child: &mut Child,
) -> Option<tokio::task::JoinHandle<()>> {
    let (out, err) = (child.stdout.take(), child.stderr.take());
    let mgr = Arc::clone(mgr);
    Some(tokio::spawn(async move {
        match (out, err) {
            (Some(o), Some(e)) => {
                tokio::join!(stream_logs(o, mgr.clone(), false), stream_logs(e, mgr, true));
            }
            (Some(o), None) => stream_logs(o, mgr, false).await,
            (None, Some(e)) => stream_logs(e, mgr, true).await,
            (None, None) => {}
        }
    }))
}

async fn stream_logs(
    reader: impl tokio::io::AsyncRead + Unpin,
    mgr: Arc<SessionManager>,
    is_stderr: bool,
) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let kind = classify_line(&line);
        if kind == LineKind::Noise {
            continue;
        }
        if is_stderr {
            if let Some(sig) = parse_signal_line(&line).or_else(|| parse_dvbstatus_line(&line)) {
                mgr.push_signal(sig).await;
            }
            if kind == LineKind::Error {
                let msg = match crate::tsduck::explain_error(&line) {
                    Some(hint) => format!("{} — {hint}", line.trim()),
                    None => line.trim().to_string(),
                };
                mgr.error(msg).await;
                continue;
            }
        }
        let _ = mgr.bus.send(WsEvent::Log { line });
    }
}

/// How a line from a job's output is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineKind {
    /// Dropped (decoder chatter, blank).
    Noise,
    /// Activity log only.
    Log,
    /// Activity log + error notification.
    Error,
}

/// Classifies tool output. Only TSDuck's own `* Error:` lines and unprefixed
/// fatal messages are errors. Lines from ffmpeg components (`[hevc @ …]`,
/// `[aist#0:1/aac @ …]`) are log-only: when a live stream is joined mid-GOP the
/// decoder reports missing references, and when tsp is stopped the last packet
/// arrives truncated — neither is a failure.
fn classify_line(line: &str) -> LineKind {
    let t = line.trim();
    const NOISE: &[&str] = &[
        "non-existing PPS",
        "no frame!",
        "sps_id",
        "PES packet size mismatch",
        "Packet corrupt",
        "corrupt input packet",
        "Last message repeated",
        "Could not find codec parameters",
        "Consider increasing the value",
        "mmco: unref short failure",
        "reference picture missing",
        "Missing reference picture",
        "Error constructing the frame RPS",
        "Could not find ref with POC",
        "Error submitting packet to decoder",
        "Invalid data found when processing input",
    ];
    if t.is_empty() || NOISE.iter().any(|n| t.contains(n)) {
        LineKind::Noise
    } else if t.starts_with("* Error") || (!t.starts_with('[') && t.contains("Error")) {
        LineKind::Error
    } else {
        LineKind::Log
    }
}

/// SIGTERM so `tsp` can flush, then SIGKILL after 5 s.
async fn graceful_stop(child: &mut Child) {
    if let Some(pid) = child.id() {
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
        if tokio::time::timeout(Duration::from_secs(5), child.wait()).await.is_ok() {
            return;
        }
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}

fn spawn_tsp(paths: &TsduckPaths, args: &[String]) -> anyhow::Result<Child> {
    let bin = paths.tool("tsp");
    let mut cmd = Command::new(&bin);
    cmd.arg("--verbose")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // tsp must not outlive the server (it would keep the tuner busy).
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    cmd.spawn()
        .with_context(|| format!("failed to start {}", bin.display()))
}

/// Reads a monitor `tsp`: analyze JSON on stdout, signal lines on stderr.
async fn read_monitor_output(
    mgr: Arc<SessionManager>,
    stdout: impl tokio::io::AsyncRead + Unpin + Send,
    stderr: impl tokio::io::AsyncRead + Unpin + Send,
) {
    let mut out = BufReader::new(stdout).lines();
    let mut err = BufReader::new(stderr).lines();
    let (mut buf, mut depth) = (String::new(), 0i32);
    let (mut out_open, mut err_open) = (true, true);
    while out_open || err_open {
        tokio::select! {
            line = out.next_line(), if out_open => match line {
                Ok(Some(line)) => {
                    if let Some(doc) = crate::analyze::feed_json_chunk(&mut buf, &mut depth, &line) {
                        let services = crate::analyze::services_from_analyze_json(&doc);
                        if !services.is_empty() {
                            mgr.push_services(services).await;
                        }
                    }
                }
                _ => out_open = false,
            },
            line = err.next_line(), if err_open => match line {
                Ok(Some(line)) => {
                    if let Some(sig) = parse_signal_line(&line).or_else(|| parse_dvbstatus_line(&line)) {
                        mgr.push_signal(sig).await;
                    }
                    if line.contains("Error") {
                        mgr.error(line.trim().to_string()).await;
                    } else {
                        let _ = mgr.bus.send(WsEvent::Log { line });
                    }
                }
                _ => err_open = false,
            },
        }
    }
}

fn now_ts() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string()
}

/// Appends log/error/milestone events to the log file.
async fn file_logger(mut rx: broadcast::Receiver<WsEvent>, path: std::path::PathBuf) {
    use tokio::io::AsyncWriteExt;
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let Ok(mut file) = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await
    else {
        eprintln!("warning: cannot open log file {}", path.display());
        return;
    };
    let _ = file
        .write_all(format!("\n===== dvbhub serve started {} =====\n", now_ts()).as_bytes())
        .await;
    loop {
        let entry = match rx.recv().await {
            Ok(WsEvent::Log { line }) => format!("[{}] {line}\n", now_ts()),
            Ok(WsEvent::Error { message }) => format!("[{}] ERROR {message}\n", now_ts()),
            Ok(WsEvent::ScanDone { locked, total, found, added, .. }) => format!(
                "[{}] >>> scan done {locked}/{total} locked, {found} found, {added} added\n",
                now_ts()
            ),
            Ok(WsEvent::CaptureDone { files, .. }) => {
                format!("[{}] >>> capture done: {}\n", now_ts(), files.join(", "))
            }
            Ok(_) => continue,
            Err(broadcast::error::RecvError::Lagged(n)) => {
                format!("[{}] (log lagged, {n} events dropped)\n", now_ts())
            }
            Err(broadcast::error::RecvError::Closed) => break,
        };
        if file.write_all(entry.as_bytes()).await.is_err() {
            break;
        }
        let _ = file.flush().await;
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_line, web_video_args, LineKind};

    #[test]
    fn decoder_chatter_is_not_an_error() {
        assert_eq!(classify_line("[hevc @ 0x556eb7340b00] Error constructing the frame RPS."), LineKind::Noise);
        assert_eq!(
            classify_line("[aist#0:1/aac @ 0x55] [dec:aac @ 0x55] Error submitting packet to decoder: Invalid data found when processing input"),
            LineKind::Noise
        );
        assert_eq!(classify_line("[hevc @ 0x1] VPS 0 does not exist"), LineKind::Log);
    }

    #[test]
    fn tsduck_errors_are_errors() {
        assert_eq!(classify_line("* Error: dvb: no input signal lock after 10000 milliseconds"), LineKind::Error);
        assert_eq!(classify_line("* dvb: locked: yes"), LineKind::Log);
        assert_eq!(classify_line("   "), LineKind::Noise);
    }

    #[test]
    fn web_quality_profiles() {
        assert!(web_video_args("720p").contains(r"min(720\,ih)"));
        assert_eq!(web_video_args("1080p"), web_video_args("720p"));
        let src = web_video_args("source");
        assert!(!src.contains("scale"));
        assert!(src.contains("superfast"));
        assert_eq!(web_video_args("bogus"), web_video_args("720p"));
    }
}


//! Live DVB session management (tune, capture, signal monitoring).
//!
//! The web UI drives short-lived `tsp` child processes for tuning and
//! recording. Tuning runs `tsp -I dvb … -P analyze --json -O drop` and parses
//! lock/strength/SNR lines plus periodic analyze JSON for the service list.
//! Capture reuses the same tuning parameters but writes to a file; stopping
//! sends SIGTERM for a clean TS finalize.

use anyhow::Context;
use crate::analyze::ServiceInfo;
use crate::config::{AppConfig, TsduckPaths};
use crate::devices::DvbDevice;
use crate::pipeline::{CaptureParams, PlayParams};
use crate::satellites::SatTransponder;
use crate::scan::{self, ScannedTransponder};
use crate::transponders;
use serde::{Deserialize, Serialize};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, Mutex, RwLock};

use crate::signal::parse_dvbstatus_line;
use crate::signal::parse_signal_line;
pub use crate::signal::SignalStatus;

#[derive(Debug, Clone, Serialize)]
pub struct SessionStatus {
    pub tuning: bool,
    pub capturing: bool,
    pub sat_capturing: bool,
    pub scanning: bool,
    pub playing: bool,
    pub playout: bool,
    pub device: Option<String>,
    pub transponder_id: Option<u32>,
    pub signal: SignalStatus,
    pub services: Vec<ServiceInfo>,
    pub scan_results: Vec<ScannedTransponder>,
    pub capture_output: Option<String>,
    pub sat_progress: Option<String>,
    pub playout_file: Option<String>,
    pub play_hls_url: Option<String>,
    pub play_service_id: Option<u64>,
    pub last_error: Option<String>,
}

impl Default for SessionStatus {
    fn default() -> Self {
        Self {
            tuning: false,
            capturing: false,
            sat_capturing: false,
            scanning: false,
            playing: false,
            playout: false,
            device: None,
            transponder_id: None,
            signal: SignalStatus::default(),
            services: Vec::new(),
            scan_results: Vec::new(),
            capture_output: None,
            sat_progress: None,
            playout_file: None,
            play_hls_url: None,
            play_service_id: None,
            last_error: None,
        }
    }
}

/// Events pushed to WebSocket subscribers.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsEvent {
    Status { status: SessionStatus },
    Signal { signal: SignalStatus },
    Services { services: Vec<ServiceInfo> },
    ScanComplete { transponders: Vec<ScannedTransponder> },
    PlayStarted { hls_url: String, service_id: u64 },
    Devices {
        devices: Vec<DvbDevice>,
        warnings: Vec<String>,
    },
    Error { message: String },
    Log { line: String },
}

/// JSON body for `POST /api/session/tune` and capture start.
#[derive(Debug, Deserialize)]
pub struct TuneRequest {
    pub device_name: Option<String>,
    pub adapter: Option<u32>,
    /// Reference transponder id (1-4); if set, manual tuning fields are ignored.
    pub transponder_id: Option<u32>,
    pub delivery_system: Option<String>,
    pub frequency_mhz: Option<u32>,
    pub polarity: Option<String>,
    pub symbol_rate_ksps: Option<u32>,
    pub modulation: Option<String>,
    pub fec: Option<String>,
    pub satellite_number: Option<u32>,
    pub signal_timeout_s: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct CaptureStartRequest {
    pub tune: TuneRequest,
    pub output: String,
    pub seconds: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct PlayStartRequest {
    pub tune: TuneRequest,
    pub service_id: u64,
}

/// A resolved plan to capture every transponder of a satellite sequentially.
pub struct SatCapturePlan {
    pub device_name: Option<String>,
    pub adapter: Option<u32>,
    pub satellite_name: String,
    pub diseqc: u32,
    pub lnb: Option<String>,
    pub seconds: u32,
    pub signal_timeout_s: u32,
    pub transponders: Vec<SatTransponder>,
}

/// JSON body for `POST /api/session/playout` (DTU-315 file -> modulator).
#[derive(Debug, Deserialize)]
pub struct PlayoutRequest {
    pub file: String,
    pub frequency_mhz: u32,
    pub modulation: String,
    pub symbol_rate_ksps: u32,
    pub code_rate: String,
    #[serde(default)]
    pub level: Option<i32>,
    #[serde(default)]
    pub infinite: bool,
}

/// Shared session manager (one tune + one capture child at a time).
pub struct SessionManager {
    paths: Arc<TsduckPaths>,
    config: Arc<RwLock<AppConfig>>,
    inner: Mutex<Inner>,
    status: RwLock<SessionStatus>,
    bus: broadcast::Sender<WsEvent>,
}

struct Inner {
    tune_child: Option<Child>,
    capture_child: Option<Child>,
    scan_child: Option<Child>,
    play_child: Option<Child>,
    playout_child: Option<Child>,
    tune_log_task: Option<tokio::task::JoinHandle<()>>,
    scan_log_task: Option<tokio::task::JoinHandle<()>>,
    capture_log_task: Option<tokio::task::JoinHandle<()>>,
    play_log_task: Option<tokio::task::JoinHandle<()>>,
    playout_log_task: Option<tokio::task::JoinHandle<()>>,
    sat_capture_task: Option<tokio::task::JoinHandle<()>>,
    sat_cancel: Arc<AtomicBool>,
    /// Incremented on each `start_play`; stale HLS watch tasks exit when mismatched.
    play_generation: u64,
    play_hls_watch: Option<tokio::task::JoinHandle<()>>,
}

impl SessionManager {
    /// Creates a new manager and returns it together with a broadcast receiver
    /// for WebSocket fan-out.
    pub fn new(
        paths: Arc<TsduckPaths>,
        config: Arc<RwLock<AppConfig>>,
    ) -> (Arc<Self>, broadcast::Receiver<WsEvent>) {
        let (tx, rx) = broadcast::channel(256);
        // Persist every log/error/action event to a file for later analysis.
        let file_log_rx = tx.subscribe();
        tokio::spawn(file_logger(file_log_rx, crate::config::log_file_path()));
        let mgr = Arc::new(Self {
            paths,
            config,
            inner: Mutex::new(Inner {
                tune_child: None,
                capture_child: None,
                scan_child: None,
                play_child: None,
                playout_child: None,
                tune_log_task: None,
                scan_log_task: None,
                capture_log_task: None,
                play_log_task: None,
                playout_log_task: None,
                sat_capture_task: None,
                sat_cancel: Arc::new(AtomicBool::new(false)),
                play_generation: 0,
                play_hls_watch: None,
            }),
            status: RwLock::new(SessionStatus::default()),
            bus: tx,
        });
        (mgr, rx)
    }

    /// Emits a high-level user-action marker to the log stream and file.
    /// These lines start with `>>>` so they stand out from tool output.
    fn action(&self, msg: impl Into<String>) {
        let _ = self.bus.send(WsEvent::Log {
            line: format!(">>> {}", msg.into()),
        });
    }

    /// Returns a clone of the broadcast sender (for additional WS clients).
    pub fn subscribe(&self) -> broadcast::Receiver<WsEvent> {
        self.bus.subscribe()
    }

    /// Returns a clone of the broadcast sender (for non-async producers like
    /// the device monitor thread).
    pub fn subscribe_sender(&self) -> broadcast::Sender<WsEvent> {
        self.bus.clone()
    }

    pub async fn status(&self) -> SessionStatus {
        self.status.read().await.clone()
    }

    /// Returns a snapshot of the current application config.
    #[allow(dead_code)]
    pub async fn config(&self) -> AppConfig {
        self.config.read().await.clone()
    }

    /// Updates the capture directory, persists the config, and creates the dir.
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
            .with_context(|| "failed to persist config")?;
        self.action(format!("SETTINGS: capture_dir = {}", cfg.capture_dir.display()));
        Ok(cfg)
    }

    async fn set_status<F>(&self, f: F)
    where
        F: FnOnce(&mut SessionStatus),
    {
        {
            let mut st = self.status.write().await;
            f(&mut st);
        }
        let st = self.status.read().await.clone();
        let _ = self.bus.send(WsEvent::Status { status: st });
    }

    async fn push_signal(&self, signal: SignalStatus) {
        {
            let mut st = self.status.write().await;
            st.signal = signal.clone();
        }
        let _ = self.bus.send(WsEvent::Signal { signal });
    }

    async fn push_services(&self, services: Vec<ServiceInfo>) {
        {
            let mut st = self.status.write().await;
            st.services = services.clone();
        }
        let _ = self.bus.send(WsEvent::Services { services });
    }

    fn emit_error(self: &Arc<Self>, message: String) {
        let _ = self.bus.send(WsEvent::Error {
            message: message.clone(),
        });
        let mgr = Arc::clone(self);
        tokio::spawn(async move {
            mgr.set_status(|st| st.last_error = Some(message)).await;
        });
    }

    /// Builds `CaptureParams` from a tune request (transponder or manual).
    pub fn capture_params_from_request(
        req: &TuneRequest,
        seconds: u32,
        output: String,
    ) -> anyhow::Result<CaptureParams> {
        if let Some(id) = req.transponder_id {
            let tp = transponders::by_id(id)
                .ok_or_else(|| anyhow::anyhow!("unknown transponder id {id}"))?;
            let mut p = CaptureParams::from_transponder(
                tp,
                req.device_name.clone(),
                req.adapter,
                seconds,
                output,
            );
            p.signal_timeout_s = req.signal_timeout_s.unwrap_or(15);
            p.satellite_number = req.satellite_number;
            return Ok(p);
        }
        let frequency_mhz = req
            .frequency_mhz
            .ok_or_else(|| anyhow::anyhow!("frequency_mhz required without transponder_id"))?;
        let delivery_system = req
            .delivery_system
            .clone()
            .ok_or_else(|| anyhow::anyhow!("delivery_system required without transponder_id"))?;
        Ok(CaptureParams {
            device_name: req.device_name.clone(),
            adapter: req.adapter,
            delivery_system,
            frequency_hz: frequency_mhz as u64 * 1_000_000,
            polarity: req.polarity.clone(),
            symbol_rate: req.symbol_rate_ksps.map(|s| s as u64 * 1_000),
            modulation: req.modulation.clone(),
            fec: req.fec.clone(),
            lnb: None,
            satellite_number: req.satellite_number,
            bandwidth_hz: None,
            guard_interval: None,
            plp: None,
            dvb_status_interval_s: None,
            signal_timeout_s: req.signal_timeout_s.unwrap_or(15),
            seconds,
            output,
            live_analyze: false,
        })
    }

    /// Starts a monitor-only tune (no file output).
    pub async fn start_tune(self: &Arc<Self>, req: TuneRequest) -> anyhow::Result<()> {
        self.stop_all().await;

        let params = Self::capture_params_from_request(&req, 0, "/dev/null".into())?;
        let mut args = params.to_tsp_args();
        // Replace file output with analyze + drop for live PSI/SI.
        if let Some(i) = args.iter().position(|a| a == "-O") {
            args.truncate(i);
        }
        args.extend([
            "-P".into(),
            "analyze".into(),
            "--json".into(),
            "--interval".into(),
            "3".into(),
            "-O".into(),
            "drop".into(),
        ]);

        let device = req
            .device_name
            .clone()
            .or_else(|| req.adapter.map(|a| format!("/dev/dvb/adapter{a}")))
            .unwrap_or_else(|| "default".into());

        self.action(format!(
            "TUNE start: device={device} transponder={}",
            req.transponder_id
                .map(|i| i.to_string())
                .unwrap_or_else(|| "manual".into())
        ));

        let mut child = spawn_tsp(&self.paths, &args).await?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("tsp tune: no stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow::anyhow!("tsp tune: no stderr"))?;

        let mgr = Arc::clone(self);
        let log_task = tokio::spawn(async move {
            read_tune_output(mgr, stdout, stderr).await;
        });

        {
            let mut inner = self.inner.lock().await;
            inner.tune_child = Some(child);
            inner.tune_log_task = Some(log_task);
        }

        self.set_status(|st| {
            st.tuning = true;
            st.capturing = false;
            st.device = Some(device);
            st.transponder_id = req.transponder_id;
            st.signal = SignalStatus::default();
            st.services.clear();
            st.capture_output = None;
            st.last_error = None;
        })
        .await;

        Ok(())
    }

    /// Stops the monitor tune child (SIGTERM).
    pub async fn stop_tune(&self) {
        let (child, task) = {
            let mut inner = self.inner.lock().await;
            (inner.tune_child.take(), inner.tune_log_task.take())
        };
        if let Some(mut c) = child {
            self.action("TUNE stop");
            graceful_stop(&mut c).await;
        }
        if let Some(t) = task {
            let _ = t.await;
        }
        self.set_status(|st| {
            st.tuning = false;
        })
        .await;
    }

    /// Starts a file capture (stops any existing tune/capture first).
    pub async fn start_capture(self: &Arc<Self>, req: CaptureStartRequest) -> anyhow::Result<()> {
        self.stop_all().await;

        let seconds = req.seconds.unwrap_or(0);
        let resolved = self.config.read().await.resolve_output(&req.output);
        if let Some(parent) = resolved.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("cannot create output dir {}", parent.display()))?;
        }
        let output = resolved.to_string_lossy().into_owned();
        let params = Self::capture_params_from_request(&req.tune, seconds, output.clone())?;
        let args = params.to_tsp_args();
        let device = req
            .tune
            .device_name
            .clone()
            .or_else(|| req.tune.adapter.map(|a| format!("/dev/dvb/adapter{a}")))
            .unwrap_or_else(|| "default".into());

        self.action(format!(
            "CAPTURE start: device={device} -> {output} ({})",
            if seconds > 0 {
                format!("{seconds}s")
            } else {
                "until stop".into()
            }
        ));

        let mut child = spawn_tsp(&self.paths, &args).await?;
        let log_task = take_child_logger(self, &mut child);
        {
            let mut inner = self.inner.lock().await;
            inner.capture_child = Some(child);
            inner.capture_log_task = log_task;
        }

        self.set_status(|st| {
            st.tuning = false;
            st.capturing = true;
            st.device = Some(device);
            st.transponder_id = req.tune.transponder_id;
            st.capture_output = Some(output);
            st.last_error = None;
        })
        .await;

        // When tsp exits on its own (--until --seconds N), reap the child and
        // notify the UI; manual stop_capture() takes the child first.
        let mgr = Arc::clone(self);
        tokio::spawn(async move {
            mgr.watch_capture_child().await;
        });

        Ok(())
    }

    /// Polls until the capture child exits, then finalizes like stop_capture.
    async fn watch_capture_child(self: &Arc<Self>) {
        loop {
            tokio::time::sleep(Duration::from_millis(400)).await;
            let exited = {
                let mut inner = self.inner.lock().await;
                match inner.capture_child.as_mut() {
                    Some(c) => matches!(c.try_wait(), Ok(Some(_))),
                    None => return,
                }
            };
            if exited {
                self.complete_capture("CAPTURE complete").await;
                return;
            }
        }
    }

    /// Reaps an already-exited capture child (timed capture finished).
    async fn complete_capture(self: &Arc<Self>, label: &str) {
        let (child, task) = {
            let mut inner = self.inner.lock().await;
            (inner.capture_child.take(), inner.capture_log_task.take())
        };
        if child.is_none() {
            return;
        }
        if let Some(mut c) = child {
            let _ = c.wait().await;
        }
        if let Some(t) = task {
            let _ = t.await;
        }
        self.finalize_capture(label).await;
    }

    /// Stops capture (SIGTERM) and optionally analyzes the output file.
    pub async fn stop_capture(self: &Arc<Self>) -> Option<String> {
        let (child, task) = {
            let mut inner = self.inner.lock().await;
            (inner.capture_child.take(), inner.capture_log_task.take())
        };
        let had_child = child.is_some();
        if let Some(mut c) = child {
            self.action("CAPTURE stop");
            graceful_stop(&mut c).await;
        }
        if let Some(t) = task {
            let _ = t.await;
        }
        let capturing = self.status.read().await.capturing;
        if had_child || capturing {
            self.finalize_capture("CAPTURE saved").await
        } else {
            self.status.read().await.capture_output.clone()
        }
    }

    /// Clears capturing flag, logs completion, and kicks off post-capture analyze.
    async fn finalize_capture(self: &Arc<Self>, label: &str) -> Option<String> {
        let output = {
            let st = self.status.read().await;
            st.capture_output.clone()
        };
        self.set_status(|st| {
            st.capturing = false;
        })
        .await;

        if let Some(ref path) = output {
            self.action(format!("{label}: {path} (analyzing…)"));
            let paths = self.paths.clone();
            let path = path.clone();
            let mgr = Arc::clone(self);
            tokio::spawn(async move {
                let services = match tokio::task::spawn_blocking(move || {
                    crate::analyze::analyze_file(&paths, &path).map(|s| s.services)
                })
                .await
                {
                    Ok(Ok(svcs)) => svcs,
                    Ok(Err(e)) => {
                        mgr.emit_error(format!("post-capture analyze failed: {e}"));
                        Vec::new()
                    }
                    Err(e) => {
                        mgr.emit_error(format!("analyze task failed: {e}"));
                        Vec::new()
                    }
                };
                if !services.is_empty() {
                    mgr.push_services(services).await;
                }
            });
        }

        output
    }

    /// Stops tune, capture, scan, play, sat-capture, and playout children.
    pub async fn stop_all(self: &Arc<Self>) {
        self.stop_sat_capture().await;
        self.stop_tune().await;
        self.stop_capture().await;
        self.stop_scan().await;
        self.stop_play().await;
        self.stop_playout().await;
    }

    /// Captures every transponder of a satellite sequentially to its own file.
    ///
    /// Runs as a background task; each transponder is tuned and recorded for
    /// `plan.seconds`. Progress is streamed to the log and reflected in status.
    /// Files are written under `<capture_dir>/<satellite>/`.
    pub async fn start_sat_capture(self: &Arc<Self>, plan: SatCapturePlan) -> anyhow::Result<()> {
        self.stop_all().await;
        if plan.transponders.is_empty() {
            anyhow::bail!("satellite has no transponders to capture");
        }

        let capture_dir = self.config.read().await.capture_dir.clone();
        let safe_name: String = plan
            .satellite_name
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '_' })
            .collect();
        let out_dir = capture_dir.join(&safe_name);
        tokio::fs::create_dir_all(&out_dir)
            .await
            .with_context(|| format!("cannot create capture dir {}", out_dir.display()))?;

        let device = plan
            .device_name
            .clone()
            .or_else(|| plan.adapter.map(|a| format!("/dev/dvb/adapter{a}")))
            .unwrap_or_else(|| "default".into());

        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut inner = self.inner.lock().await;
            inner.sat_cancel = cancel.clone();
        }

        let total = plan.transponders.len();
        self.action(format!(
            "SAT capture start: {} ({total} TPs, {}s each) -> {}",
            plan.satellite_name,
            plan.seconds,
            out_dir.display()
        ));
        self.set_status(|st| {
            st.tuning = false;
            st.capturing = false;
            st.sat_capturing = true;
            st.device = Some(device.clone());
            st.sat_progress = Some(format!("0/{total}"));
            st.last_error = None;
        })
        .await;

        let mgr = Arc::clone(self);
        let task = tokio::spawn(async move {
            mgr.run_sat_capture(plan, out_dir, total, cancel).await;
        });
        {
            let mut inner = self.inner.lock().await;
            inner.sat_capture_task = Some(task);
        }
        Ok(())
    }

    /// Body of the sequential satellite-capture task.
    async fn run_sat_capture(
        self: Arc<Self>,
        plan: SatCapturePlan,
        out_dir: std::path::PathBuf,
        total: usize,
        cancel: Arc<AtomicBool>,
    ) {
        // Slug for file names, matching the TP-capture convention
        // (`<satname>_<freq><pol><sr>_<timestamp>.ts`).
        let sat_slug: String = plan
            .satellite_name
            .to_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();
        let sat_slug = if sat_slug.is_empty() {
            "sat".to_string()
        } else {
            sat_slug
        };

        let mut done = 0usize;
        for (idx, tp) in plan.transponders.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let pol = tp.polarity.chars().next().unwrap_or('x').to_ascii_uppercase();
            let pol_lc = pol.to_ascii_lowercase();
            let stamp = chrono::Local::now().format("%Y%m%d%H%M%S");
            let file = out_dir.join(format!(
                "{}_{}{}{}_{}.ts",
                sat_slug, tp.frequency_mhz, pol_lc, tp.symbol_rate_ksps, stamp
            ));
            let params = CaptureParams {
                device_name: plan.device_name.clone(),
                adapter: plan.adapter,
                delivery_system: tp.delivery_system.clone(),
                frequency_hz: tp.frequency_mhz as u64 * 1_000_000,
                polarity: Some(tp.polarity.clone()),
                symbol_rate: Some(tp.symbol_rate_ksps as u64 * 1_000),
                modulation: Some(tp.modulation.clone()),
                fec: Some(tp.fec.clone()),
                lnb: plan.lnb.clone(),
                satellite_number: Some(plan.diseqc),
                bandwidth_hz: None,
                guard_interval: None,
                plp: None,
                dvb_status_interval_s: None,
                signal_timeout_s: plan.signal_timeout_s,
                seconds: plan.seconds,
                output: file.to_string_lossy().into_owned(),
                live_analyze: false,
            };
            let progress = format!("{}/{}", idx + 1, total);
            self.set_status(|st| st.sat_progress = Some(progress.clone())).await;
            self.action(format!(
                "SAT capture {}/{}: {} MHz {} {} -> {}",
                idx + 1,
                total,
                tp.frequency_mhz,
                pol,
                tp.delivery_system,
                file.display()
            ));

            let args = params.to_tsp_args();
            let mut child = match spawn_tsp(&self.paths, &args).await {
                Ok(c) => c,
                Err(e) => {
                    self.emit_error(format!("SAT capture spawn failed: {e}"));
                    continue;
                }
            };
            let log_task = take_child_logger(&self, &mut child);
            {
                let mut inner = self.inner.lock().await;
                inner.capture_child = Some(child);
                inner.capture_log_task = log_task;
            }

            // Wait for this transponder's capture to finish (or be cancelled).
            loop {
                if cancel.load(Ordering::Relaxed) {
                    self.stop_capture().await;
                    break;
                }
                let finished = {
                    let mut inner = self.inner.lock().await;
                    match inner.capture_child.as_mut() {
                        Some(c) => matches!(c.try_wait(), Ok(Some(_))),
                        None => true,
                    }
                };
                if finished {
                    self.stop_capture().await;
                    done += 1;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(400)).await;
            }
        }

        let cancelled = cancel.load(Ordering::Relaxed);
        self.action(format!(
            "SAT capture {}: {done}/{total} transponders recorded",
            if cancelled { "stopped" } else { "complete" }
        ));
        self.set_status(|st| {
            st.sat_capturing = false;
            st.sat_progress = None;
        })
        .await;
    }

    /// Stops an in-progress satellite capture (cancels the loop + current TP).
    pub async fn stop_sat_capture(self: &Arc<Self>) {
        let (task, cancel) = {
            let inner = self.inner.lock().await;
            (
                inner.sat_capture_task.is_some(),
                inner.sat_cancel.clone(),
            )
        };
        if !task {
            return;
        }
        cancel.store(true, Ordering::Relaxed);
        // Kill the in-flight per-TP capture so the loop advances immediately.
        let child = {
            let mut inner = self.inner.lock().await;
            inner.capture_child.take()
        };
        if let Some(mut c) = child {
            graceful_stop(&mut c).await;
        }
        let handle = {
            let mut inner = self.inner.lock().await;
            inner.sat_capture_task.take()
        };
        if let Some(h) = handle {
            let _ = h.await;
        }
        self.set_status(|st| {
            st.sat_capturing = false;
            st.sat_progress = None;
        })
        .await;
    }

    /// Starts a DTU-315 playout of a transport-stream file.
    pub async fn start_playout(self: &Arc<Self>, req: PlayoutRequest) -> anyhow::Result<()> {
        self.stop_all().await;

        let resolved = self.config.read().await.resolve_output(&req.file);
        if !resolved.is_file() {
            anyhow::bail!("playout file not found: {}", resolved.display());
        }
        let file = resolved.to_string_lossy().into_owned();
        let params = PlayParams {
            input: file.clone(),
            infinite: req.infinite,
            delivery: crate::pipeline::PlayoutDelivery::Satellite,
            frequency_hz: req.frequency_mhz as u64 * 1_000_000,
            modulation: req.modulation.clone(),
            symbol_rate: Some(req.symbol_rate_ksps as u64 * 1_000),
            code_rate: Some(req.code_rate.clone()),
            bandwidth_hz: None,
            guard_interval: None,
            plp_modulation: None,
            j83: None,
            level_dbm_tenths: req.level,
            instant_detach: !req.infinite,
        };
        let args = params.to_tsp_args();

        self.action(format!(
            "PLAYOUT start: {} @ {} MHz {} {}kS/s {}",
            file, req.frequency_mhz, req.modulation, req.symbol_rate_ksps, req.code_rate
        ));

        let mut child = spawn_tsp(&self.paths, &args).await?;
        let log_task = take_child_logger(self, &mut child);
        {
            let mut inner = self.inner.lock().await;
            inner.playout_child = Some(child);
            inner.playout_log_task = log_task;
        }
        self.set_status(|st| {
            st.playout = true;
            st.playout_file = Some(file);
            st.last_error = None;
        })
        .await;
        Ok(())
    }

    /// Stops a DTU-315 playout.
    pub async fn stop_playout(&self) {
        let (child, task) = {
            let mut inner = self.inner.lock().await;
            (inner.playout_child.take(), inner.playout_log_task.take())
        };
        if let Some(mut c) = child {
            self.action("PLAYOUT stop");
            graceful_stop(&mut c).await;
        }
        if let Some(t) = task {
            let _ = t.await;
        }
        self.set_status(|st| {
            st.playout = false;
            st.playout_file = None;
        })
        .await;
    }

    /// Runs `tsscan --nit-scan` on the tuned transponder and parses NIT entries.
    pub async fn start_nit_scan(self: &Arc<Self>, req: TuneRequest) -> anyhow::Result<()> {
        self.stop_all().await;

        let params = Self::capture_params_from_request(&req, 0, String::new())?;
        let args = tsscan_args_from_params(&params);
        let device = req
            .device_name
            .clone()
            .or_else(|| req.adapter.map(|a| format!("/dev/dvb/adapter{a}")))
            .unwrap_or_else(|| "default".into());

        self.action(format!("NIT scan start: device={device}"));

        let mut child = spawn_tsscan(&self.paths, &args).await?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("tsscan: no stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow::anyhow!("tsscan: no stderr"))?;

        {
            let mut inner = self.inner.lock().await;
            inner.scan_child = Some(child);
        }

        self.set_status(|st| {
            st.tuning = false;
            st.capturing = false;
            st.playing = false;
            st.scanning = true;
            st.device = Some(device);
            st.transponder_id = req.transponder_id;
            st.scan_results.clear();
            st.last_error = None;
        })
        .await;

        let mgr = Arc::clone(self);
        let task = tokio::spawn(async move {
            let output = read_process_output(mgr.clone(), stdout, stderr).await;
            let mut child = mgr.inner.lock().await.scan_child.take();
            if let Some(ref mut c) = child {
                if let Ok(status) = c.wait().await {
                    if !status.success() {
                        mgr.emit_error(format!("tsscan exited with {status}"));
                    }
                }
            }
            let results = scan::parse_tsscan_output(&output);
            mgr.set_status(|st| {
                st.scanning = false;
                st.scan_results = results.clone();
            })
            .await;
            let _ = mgr.bus.send(WsEvent::ScanComplete {
                transponders: results,
            });
        });

        {
            let mut inner = self.inner.lock().await;
            inner.scan_log_task = Some(task);
        }

        Ok(())
    }

    /// Stops an in-progress NIT scan.
    pub async fn stop_scan(&self) {
        let (child, task) = {
            let mut inner = self.inner.lock().await;
            (inner.scan_child.take(), inner.scan_log_task.take())
        };
        if let Some(mut c) = child {
            self.action("NIT scan stop");
            graceful_stop(&mut c).await;
        }
        if let Some(t) = task {
            let _ = t.await;
        }
        self.set_status(|st| {
            st.scanning = false;
        })
        .await;
    }

    /// Starts live FTA playback via `zap` + ffmpeg HLS fork.
    pub async fn start_play(self: &Arc<Self>, req: PlayStartRequest) -> anyhow::Result<String> {
        self.stop_all().await;

        let hls_root = self.config.read().await.hls_dir();
        // Each service gets its own subdirectory so channel switches never reuse
        // a stale manifest/segments from the previous zap target.
        let hls_dir = hls_root.join(format!("s{}", req.service_id));
        tokio::fs::create_dir_all(&hls_dir)
            .await
            .with_context(|| format!("cannot create HLS dir {}", hls_dir.display()))?;
        clean_hls_dir(&hls_dir).await;
        let m3u8_path = hls_dir.join("stream.m3u8");
        let m3u8_str = m3u8_path.to_string_lossy().into_owned();

        let params = Self::capture_params_from_request(&req.tune, 0, String::new())?;
        let mut args = params.to_tsp_args();
        if let Some(i) = args.iter().position(|a| a == "-O") {
            args.truncate(i);
        }
        // Reduce the TS to the chosen service, then hand it to ffmpeg which
        // remuxes to HLS. Video is copied (H.264); audio is transcoded to AAC
        // because MPEG-1/2 audio is not playable via HLS in most browsers.
        // Only the video+audio elementary streams are mapped (SCTE-35, DVB
        // subtitles and data PIDs would break the HLS muxer).
        // Latency tuning: keep the input probe short and segments small so the
        // first playable keyframe/manifest appears quickly (the player joins the
        // live edge after a couple of 1 s segments).
        let ffmpeg = format!(
            "ffmpeg -hide_banner -loglevel error -fflags +genpts+discardcorrupt+nobuffer -flags low_delay -analyzeduration 1M -probesize 1M -i pipe:0 -map 0:v:0 -map 0:a:0? -c:v copy -c:a aac -b:a 128k -ac 2 -f hls -hls_time 1 -hls_list_size 6 -hls_flags delete_segments+append_list+omit_endlist -hls_segment_type mpegts {m3u8_str}"
        );
        args.extend([
            "-P".into(),
            "zap".into(),
            req.service_id.to_string(),
            "-O".into(),
            "fork".into(),
            ffmpeg,
        ]);

        let device = req
            .tune
            .device_name
            .clone()
            .or_else(|| req.tune.adapter.map(|a| format!("/dev/dvb/adapter{a}")))
            .unwrap_or_else(|| "default".into());
        let hls_url = format!("/hls/s{}/stream.m3u8", req.service_id);

        self.action(format!(
            "PLAY start: device={device} service={}",
            req.service_id
        ));

        let mut child = spawn_tsp(&self.paths, &args).await?;
        let log_task = take_child_logger(self, &mut child);
        {
            let mut inner = self.inner.lock().await;
            inner.play_child = Some(child);
            inner.play_log_task = log_task;
        }

        self.set_status(|st| {
            st.tuning = false;
            st.capturing = false;
            st.scanning = false;
            st.playing = true;
            st.device = Some(device);
            st.transponder_id = req.tune.transponder_id;
            st.play_hls_url = Some(hls_url.clone());
            st.play_service_id = Some(req.service_id);
            st.last_error = None;
        })
        .await;

        // Notify the UI only once the HLS manifest exists (ffmpeg needs ~10-15 s
        // to lock, zap and reach the first keyframe). Cancel any prior watcher so
        // channel switches do not emit stale PlayStarted events.
        let mgr = Arc::clone(self);
        let m3u8_watch = m3u8_path.clone();
        let sid = req.service_id;
        let url = hls_url.clone();
        let generation = {
            let mut inner = self.inner.lock().await;
            if let Some(h) = inner.play_hls_watch.take() {
                h.abort();
            }
            inner.play_generation += 1;
            inner.play_generation
        };
        let watch = tokio::spawn(async move {
            for _ in 0..45 {
                let still_current = {
                    let inner = mgr.inner.lock().await;
                    inner.play_generation == generation
                };
                if !still_current {
                    return;
                }
                if tokio::fs::metadata(&m3u8_watch).await.is_ok() {
                    let still_current = {
                        let inner = mgr.inner.lock().await;
                        inner.play_generation == generation
                    };
                    if still_current {
                        let _ = mgr.bus.send(WsEvent::PlayStarted {
                            hls_url: url,
                            service_id: sid,
                        });
                    }
                    return;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            let still_current = {
                let inner = mgr.inner.lock().await;
                inner.play_generation == generation
            };
            if still_current {
                mgr.emit_error(
                    "HLS manifest not created after 45 s — check ~/.config/dvbhub/dvbhub.log"
                        .into(),
                );
            }
        });
        {
            let mut inner = self.inner.lock().await;
            inner.play_hls_watch = Some(watch);
        }

        Ok(hls_url)
    }

    /// Stops live playback.
    pub async fn stop_play(&self) {
        {
            let mut inner = self.inner.lock().await;
            inner.play_generation += 1;
            if let Some(h) = inner.play_hls_watch.take() {
                h.abort();
            }
        }
        let (child, task) = {
            let mut inner = self.inner.lock().await;
            (inner.play_child.take(), inner.play_log_task.take())
        };
        if let Some(mut c) = child {
            self.action("PLAY stop");
            graceful_stop(&mut c).await;
        }
        if let Some(t) = task {
            let _ = t.await;
        }
        let hls_root = self.config.read().await.hls_dir();
        clean_hls_tree(&hls_root).await;
        self.set_status(|st| {
            st.playing = false;
            st.play_hls_url = None;
            st.play_service_id = None;
        })
        .await;
    }
}

/// Removes stale HLS artefacts (`*.ts`, `*.m3u8`, `*.vtt`) in one directory.
async fn clean_hls_dir(dir: &std::path::Path) {
    if let Ok(mut entries) = tokio::fs::read_dir(dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let remove = matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("ts") | Some("m3u8") | Some("vtt")
            );
            if remove {
                let _ = tokio::fs::remove_file(&path).await;
            }
        }
    }
}

/// Removes all per-service HLS subdirectories under the HLS root.
async fn clean_hls_tree(root: &std::path::Path) {
    if let Ok(mut entries) = tokio::fs::read_dir(root).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.is_dir() {
                let _ = tokio::fs::remove_dir_all(&path).await;
            } else {
                let remove = matches!(
                    path.extension().and_then(|e| e.to_str()),
                    Some("ts") | Some("m3u8") | Some("vtt")
                );
                if remove {
                    let _ = tokio::fs::remove_file(&path).await;
                }
            }
        }
    }
}

/// Takes a child's stdout/stderr and spawns a task that streams every line to
/// WebSocket log subscribers (and surfaces errors). Returns the task handle.
fn take_child_logger(
    mgr: &Arc<SessionManager>,
    child: &mut Child,
) -> Option<tokio::task::JoinHandle<()>> {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    if stdout.is_none() && stderr.is_none() {
        return None;
    }
    let mgr = Arc::clone(mgr);
    Some(tokio::spawn(async move {
        match (stdout, stderr) {
            (Some(o), Some(e)) => {
                tokio::join!(stream_logs(o, mgr.clone(), false), stream_logs(e, mgr, true));
            }
            (Some(o), None) => stream_logs(o, mgr, false).await,
            (None, Some(e)) => stream_logs(e, mgr, true).await,
            (None, None) => {}
        }
    }))
}

/// Streams a reader line-by-line to the log bus without buffering the whole
/// output; flags lines containing "error" as session errors.
async fn stream_logs(
    reader: impl tokio::io::AsyncRead + Unpin,
    mgr: Arc<SessionManager>,
    is_stderr: bool,
) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if is_noise_line(&line) {
            continue;
        }
        if is_stderr {
            if let Some(sig) = parse_signal_line(&line).or_else(|| parse_dvbstatus_line(&line)) {
                mgr.push_signal(sig).await;
            }
            if line.contains("error") || line.contains("Error") {
                mgr.emit_error(line.clone());
            }
        }
        if !line.trim().is_empty() {
            let _ = mgr.bus.send(WsEvent::Log { line });
        }
    }
}

/// Transient ffmpeg/decoder chatter that floods the log when a live stream is
/// joined mid-GOP (before the first keyframe / PMT). These are not real errors;
/// dropping them keeps the activity log readable.
fn is_noise_line(line: &str) -> bool {
    let t = line.trim();
    t.contains("non-existing PPS")
        || t.contains("no frame!")
        || t.contains("sps_id")
        || t.contains("PES packet size mismatch")
        || t.contains("Packet corrupt")
        || t.contains("corrupt input packet")
        || t.starts_with("Last message repeated")
        || t.contains("Could not find codec parameters")
        || t.contains("Consider increasing the value")
}

/// Sends SIGTERM first so `tsp` can flush outputs, then SIGKILL if needed.
async fn graceful_stop(child: &mut Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        let _ = std::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status();
        let _ = tokio::time::timeout(Duration::from_secs(8), child.wait()).await;
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}

async fn spawn_tsp(paths: &TsduckPaths, args: &[String]) -> anyhow::Result<Child> {
    let bin = paths.tool("tsp");
    let child = Command::new(&bin)
        .arg("--verbose")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("failed to spawn {}", bin.display()))?;
    Ok(child)
}

async fn read_tune_output(
    mgr: Arc<SessionManager>,
    stdout: impl tokio::io::AsyncRead + Unpin + Send,
    stderr: impl tokio::io::AsyncRead + Unpin + Send,
) {
    let mut stdout_lines = BufReader::new(stdout).lines();
    let mut stderr_lines = BufReader::new(stderr).lines();
    let mut json_buf = String::new();
    let mut json_depth: i32 = 0;

    loop {
        tokio::select! {
            line = stdout_lines.next_line() => {
                match line {
                    Ok(Some(line)) => {
                        if let Some(doc) = feed_json_chunk(&mut json_buf, &mut json_depth, &line) {
                            try_parse_analyze_json(&mgr, &doc).await;
                        }
                        if json_depth == 0
                            && !line.trim().is_empty()
                            && !is_analyze_json_line(&line)
                        {
                            let _ = mgr.bus.send(WsEvent::Log { line });
                        }
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            line = stderr_lines.next_line() => {
                match line {
                    Ok(Some(line)) => {
                        if let Some(sig) = parse_signal_line(&line).or_else(|| parse_dvbstatus_line(&line)) {
                            mgr.push_signal(sig).await;
                        }
                        if line.contains("error") || line.contains("Error") {
                            mgr.emit_error(line.clone());
                        }
                        let _ = mgr.bus.send(WsEvent::Log { line });
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
        }
    }
}

/// Accumulates stdout until a complete top-level JSON object is seen.
fn feed_json_chunk(buf: &mut String, depth: &mut i32, chunk: &str) -> Option<String> {
    for ch in chunk.chars() {
        if *depth == 0 && ch != '{' {
            continue;
        }
        buf.push(ch);
        match ch {
            '{' => *depth += 1,
            '}' => {
                *depth -= 1;
                if *depth == 0 {
                    let doc = buf.clone();
                    buf.clear();
                    return Some(doc);
                }
            }
            _ => {}
        }
    }
    None
}

async fn read_process_output(
    mgr: Arc<SessionManager>,
    stdout: impl tokio::io::AsyncRead + Unpin + Send,
    stderr: impl tokio::io::AsyncRead + Unpin + Send,
) -> String {
    let (out, err) = tokio::join!(
        read_stream_to_string(stdout, mgr.clone()),
        read_stream_to_string(stderr, mgr),
    );
    format!("{out}{err}")
}

async fn read_stream_to_string(
    mut reader: impl tokio::io::AsyncRead + Unpin,
    mgr: Arc<SessionManager>,
) -> String {
    use tokio::io::AsyncReadExt;
    let mut buf = Vec::new();
    if reader.read_to_end(&mut buf).await.is_ok() {
        let text = String::from_utf8_lossy(&buf).into_owned();
        for line in text.lines() {
            if !line.trim().is_empty() {
                let _ = mgr.bus.send(WsEvent::Log {
                    line: line.to_string(),
                });
            }
        }
        text
    } else {
        String::new()
    }
}

async fn try_parse_analyze_json(mgr: &Arc<SessionManager>, text: &str) {
    let services = crate::analyze::services_from_analyze_json(text);
    if !services.is_empty() {
        mgr.push_services(services).await;
    }
}

fn tsscan_args_from_params(p: &CaptureParams) -> Vec<String> {
    let mut a = vec!["--nit-scan".to_string()];
    if let Some(name) = &p.device_name {
        a.push("--device-name".into());
        a.push(name.clone());
    }
    if let Some(ad) = p.adapter {
        a.push("--adapter".into());
        a.push(ad.to_string());
    }
    a.push("--delivery-system".into());
    a.push(p.delivery_system.clone());
    a.push("--frequency".into());
    a.push(p.frequency_hz.to_string());
    if let Some(pol) = &p.polarity {
        a.push("--polarity".into());
        a.push(pol.clone());
    }
    if let Some(sr) = p.symbol_rate {
        a.push("--symbol-rate".into());
        a.push(sr.to_string());
    }
    if let Some(m) = &p.modulation {
        a.push("--modulation".into());
        a.push(m.clone());
    }
    if let Some(f) = &p.fec {
        a.push("--fec-inner".into());
        a.push(f.clone());
    }
    if let Some(sn) = p.satellite_number {
        a.push("--satellite-number".into());
        a.push(sn.to_string());
    }
    a.push("--signal-timeout".into());
    a.push(p.signal_timeout_s.to_string());
    a
}

async fn spawn_tsscan(paths: &TsduckPaths, args: &[String]) -> anyhow::Result<Child> {
    let bin = paths.tool("tsscan");
    let child = Command::new(&bin)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("failed to spawn {}", bin.display()))?;
    Ok(child)
}

/// True for lines that belong to `tsp -P analyze --json` output (not tool logs).
fn is_analyze_json_line(s: &str) -> bool {
    let t = s.trim();
    t.starts_with('{')
        || t == "}"
        || t == "},"
        || t == "]"
        || t == "],"
        || (t.starts_with('"') && t.contains(':'))
        || (t.starts_with("  \"") && t.contains(':'))
}

/// Current local time as `YYYY-MM-DD HH:MM:SS.mmm` for log prefixes.
fn now_ts() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string()
}

/// Appends every log/error/action event to a file with timestamps so the user
/// can review which UI actions were taken and what the tools reported.
async fn file_logger(mut rx: broadcast::Receiver<WsEvent>, path: std::path::PathBuf) {
    use tokio::io::AsyncWriteExt;
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let mut file = match tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await
    {
        Ok(f) => f,
        Err(e) => {
            eprintln!("warning: cannot open log file {}: {e}", path.display());
            return;
        }
    };
    let _ = file
        .write_all(format!("\n===== dvbhub started {} =====\n", now_ts()).as_bytes())
        .await;
    let _ = file.flush().await;

    loop {
        match rx.recv().await {
            Ok(ev) => {
                let entry = match ev {
                    WsEvent::Log { line } => Some(format!("[{}] {}\n", now_ts(), line)),
                    WsEvent::Error { message } => {
                        Some(format!("[{}] ERROR {}\n", now_ts(), message))
                    }
                    WsEvent::ScanComplete { transponders } => Some(format!(
                        "[{}] >>> NIT scan complete: {} transponders\n",
                        now_ts(),
                        transponders.len()
                    )),
                    WsEvent::PlayStarted { service_id, .. } => Some(format!(
                        "[{}] >>> PLAY ready: service {} (HLS)\n",
                        now_ts(),
                        service_id
                    )),
                    // Status/Signal/Services are high-frequency; skip in file.
                    _ => None,
                };
                if let Some(line) = entry {
                    if file.write_all(line.as_bytes()).await.is_err() {
                        break;
                    }
                    let _ = file.flush().await;
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                let _ = file
                    .write_all(
                        format!("[{}] (log lagged, {n} events dropped)\n", now_ts()).as_bytes(),
                    )
                    .await;
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

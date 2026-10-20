//! Local web interface for dvbhub (`dvbhub serve`).
//!
//! A small axum server on localhost exposes the same engine as the CLI/TUI:
//! device discovery (with a hot-plug monitor), capture, channel scan, live
//! watch in the browser (HLS), DTU-315 playout and the satellite / reference
//! databases. Static UI files are embedded in the binary; progress is pushed
//! over a WebSocket.

use crate::analyze::{self, CaptureServicesRecord};
use crate::cable::CableMux;
use crate::channel_scan::{self, ScanOptions};
use crate::channels::{ChannelDb, ChannelDelivery};
use crate::config::{AppConfig, TsduckPaths};
use crate::devices::{DeliveryCategory, Discovery, DvbDevice};
use crate::lastlog;
use crate::monitor::{self, DeviceState};
use crate::pipeline::CaptureParams;
use crate::refdb::{ReferenceCapture, ReferenceCaptureDb, ReferenceTpDb};
use crate::satellites::{tsp_lnb_value, SatTransponder, Satellite, SatelliteDb};
use crate::session::{self, CaptureJob, PlayJob, ScanJob, SessionManager, WsEvent};
use crate::terrestrial::TerrestrialMux;
use crate::tsduck;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

/// Shared application state handed to every request handler.
#[derive(Clone)]
struct AppState {
    paths: Arc<TsduckPaths>,
    config: Arc<RwLock<AppConfig>>,
    sessions: Arc<SessionManager>,
    device_state: DeviceState,
    satellites: Arc<RwLock<SatelliteDb>>,
    ref_tps: Arc<RwLock<ReferenceTpDb>>,
    ref_caps: Arc<RwLock<ReferenceCaptureDb>>,
    channels: Arc<RwLock<ChannelDb>>,
}

/// Embedded static files: (URL path, content type, body).
const STATIC_FILES: &[(&str, &str, &str)] = &[
    ("app.css", "text/css; charset=utf-8", include_str!("../web/app.css")),
    ("hls.js", "application/javascript", include_str!("../web/hls.min.js")),
    ("js/api.js", "application/javascript", include_str!("../web/js/api.js")),
    ("js/ui.js", "application/javascript", include_str!("../web/js/ui.js")),
    ("js/main.js", "application/javascript", include_str!("../web/js/main.js")),
    ("js/devices.js", "application/javascript", include_str!("../web/js/devices.js")),
    ("js/capture.js", "application/javascript", include_str!("../web/js/capture.js")),
    ("js/playback.js", "application/javascript", include_str!("../web/js/playback.js")),
    ("js/watch.js", "application/javascript", include_str!("../web/js/watch.js")),
];

const INDEX_HTML: &str = include_str!("../web/index.html");

/// Runs the web server until Ctrl-C (blocking; builds its own runtime).
pub fn serve_blocking(bind: &str, port: u16) -> anyhow::Result<()> {
    lastlog::event("cmd.serve", &format!("bind={bind} port={port}"));
    let paths = TsduckPaths::resolve()?;
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(serve(paths, bind, port))
}

async fn serve(paths: TsduckPaths, bind: &str, port: u16) -> anyhow::Result<()> {
    let paths = Arc::new(paths);
    let cfg = AppConfig::load();
    if tokio::fs::create_dir_all(&cfg.capture_dir).await.is_err() {
        eprintln!("warning: could not create capture dir {}", cfg.capture_dir.display());
    }
    let _ = cfg.save();
    println!("config file: {}", crate::config::config_file_path().display());
    println!("capture dir: {}", cfg.capture_dir.display());
    println!("log file:    {}", crate::config::log_file_path().display());
    println!("HLS dir:     {}", session::hls_root().display());

    let config = Arc::new(RwLock::new(cfg));
    let channels = Arc::new(RwLock::new(ChannelDb::load()));
    let sessions = SessionManager::new(paths.clone(), config.clone(), channels.clone());
    let device_state: DeviceState = Arc::new(std::sync::RwLock::new(Discovery {
        devices: Vec::new(),
        warnings: Vec::new(),
    }));
    monitor::spawn(paths.clone(), device_state.clone(), sessions.sender());

    let state = AppState {
        paths,
        config,
        sessions,
        device_state,
        satellites: Arc::new(RwLock::new(SatelliteDb::load())),
        ref_tps: Arc::new(RwLock::new(ReferenceTpDb::load())),
        ref_caps: Arc::new(RwLock::new(ReferenceCaptureDb::load())),
        channels,
    };

    let app = Router::new()
        .route("/", get(|| async { Html(INDEX_HTML) }))
        .route("/static/*path", get(static_file))
        .route("/api/health", get(health))
        .route("/api/devices", get(api_devices))
        .route("/api/config", get(api_config_get).post(api_config_set))
        .route("/api/analyze", get(api_analyze))
        .route("/api/captures", get(api_captures))
        .route("/api/playout/suggest", get(api_playout_suggest))
        .route("/api/satellites", get(api_satellites_get).post(api_satellites_upsert))
        .route("/api/satellites/import", post(api_satellites_import))
        .route("/api/satellites/:name", delete(api_satellites_delete))
        .route("/api/reference-transponders", get(api_ref_tps_get).post(api_ref_tps_add))
        .route("/api/reference-transponders/:index", delete(api_ref_tps_delete))
        .route("/api/reference-captures", get(api_ref_caps_get).post(api_ref_caps_upsert))
        .route("/api/reference-captures/:name", delete(api_ref_caps_delete))
        .route("/api/channels", get(api_channels_get).delete(api_channels_clear))
        .route("/api/channels/:id", delete(api_channels_delete))
        .route("/api/session/status", get(api_status))
        .route("/api/session/tuner", delete(api_tuner_stop))
        .route("/api/session/monitor", post(api_monitor))
        .route("/api/session/capture", post(api_capture))
        .route("/api/session/scan", post(api_scan))
        .route("/api/session/play", post(api_play))
        .route("/api/session/playout", post(api_playout).delete(api_playout_stop))
        .route("/hls/*path", get(hls_file))
        .route("/ws", get(ws_upgrade))
        .with_state(state.clone());

    let addr = format!("{bind}:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    println!("dvbhub web interface listening on http://{addr}  (Ctrl-C to stop)");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    println!("\nshutting down: stopping tuner and playout jobs");
    state.sessions.stop_tuner().await;
    state.sessions.stop_playout().await;
    Ok(())
}

// ---- helpers ------------------------------------------------------------

/// JSON error response.
fn error_json(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "ok": false, "error": message }))).into_response()
}

fn bad_request(e: impl std::fmt::Display) -> Response {
    error_json(StatusCode::BAD_REQUEST, &e.to_string())
}

fn ok() -> Response {
    Json(json!({ "ok": true })).into_response()
}

fn discovery(state: &AppState) -> Discovery {
    state.device_state.read().map(|d| d.clone()).unwrap_or(Discovery {
        devices: Vec::new(),
        warnings: Vec::new(),
    })
}

fn parse_category(s: &str) -> anyhow::Result<DeliveryCategory> {
    match s.to_ascii_lowercase().as_str() {
        "satellite" | "sat" | "s" => Ok(DeliveryCategory::Satellite),
        "terrestrial" | "ter" | "t" => Ok(DeliveryCategory::Terrestrial),
        "cable" | "cab" | "c" => Ok(DeliveryCategory::Cable),
        other => anyhow::bail!("unknown delivery {other:?} (satellite / terrestrial / cable)"),
    }
}

/// Picks the tuner for `cat`: the requested device (by address) or the best match.
/// Returns `(capture address, device label)`.
fn resolve_tuner(
    state: &AppState,
    cat: DeliveryCategory,
    device_name: Option<&str>,
    model: Option<&str>,
) -> anyhow::Result<(String, String)> {
    let disc = discovery(state);
    let dev: Option<&DvbDevice> = match device_name.filter(|d| !d.is_empty()) {
        Some(addr) => disc
            .devices
            .iter()
            .find(|d| d.address == addr || d.capture_address(cat) == addr),
        None => disc.tuner_for(cat, model),
    };
    let dev = dev.ok_or_else(|| anyhow::anyhow!("no {} tuner available", cat.as_str()))?;
    anyhow::ensure!(
        dev.supports_category(cat),
        "{} does not support {}",
        dev.label,
        cat.as_str()
    );
    Ok((dev.capture_address(cat), dev.label.clone()))
}

/// Persists a JSON database off the async runtime (best effort, logged).
fn persist(what: &'static str, f: impl FnOnce() -> std::io::Result<()> + Send + 'static) {
    tokio::task::spawn_blocking(move || {
        if let Err(e) = f() {
            lastlog::event("web.error", &format!("saving {what} failed: {e}"));
        }
    });
}

// ---- static + misc --------------------------------------------------------

async fn static_file(AxumPath(path): AxumPath<String>) -> Response {
    match STATIC_FILES.iter().find(|(p, _, _)| *p == path) {
        Some((_, ct, body)) => ([(header::CONTENT_TYPE, *ct)], *body).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn health(State(state): State<AppState>) -> Response {
    let paths = state.paths.clone();
    let version = tokio::task::spawn_blocking(move || tsduck::version(&paths))
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "unknown".into());
    Json(json!({ "ok": true, "tsduck_version": version, "dvbhub_version": env!("CARGO_PKG_VERSION") }))
        .into_response()
}

async fn api_devices(State(state): State<AppState>) -> Response {
    Json(discovery(&state)).into_response()
}

async fn api_config_get(State(state): State<AppState>) -> Response {
    let cfg = state.config.read().await.clone();
    Json(json!({
        "ok": true,
        "capture_dir": cfg.capture_dir.to_string_lossy(),
        "capture_seconds": cfg.capture_seconds,
        "config_file": crate::config::config_file_path().to_string_lossy(),
        "log_file": crate::config::log_file_path().to_string_lossy(),
        "hls_dir": session::hls_root().to_string_lossy(),
    }))
    .into_response()
}

#[derive(Deserialize)]
struct ConfigUpdate {
    capture_dir: String,
}

async fn api_config_set(State(state): State<AppState>, Json(req): Json<ConfigUpdate>) -> Response {
    let dir = AppConfig::expand_path(&req.capture_dir);
    if req.capture_dir.trim().is_empty() {
        return bad_request("capture_dir cannot be empty");
    }
    match state.sessions.set_capture_dir(dir).await {
        Ok(cfg) => Json(json!({ "ok": true, "capture_dir": cfg.capture_dir.to_string_lossy() }))
            .into_response(),
        Err(e) => bad_request(e),
    }
}

#[derive(Deserialize)]
struct FileQuery {
    file: String,
}

async fn api_analyze(State(state): State<AppState>, Query(q): Query<FileQuery>) -> Response {
    let path = state.config.read().await.resolve_output(&q.file);
    let paths = state.paths.clone();
    let file = path.to_string_lossy().into_owned();
    match tokio::task::spawn_blocking(move || analyze::analyze_file(&paths, &file)).await {
        Ok(Ok(summary)) => Json(summary).into_response(),
        Ok(Err(e)) => error_json(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        Err(e) => error_json(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

/// Lists `.ts` files under the capture dir (newest first) with sidecar info.
async fn api_captures(State(state): State<AppState>) -> Response {
    let root = state.config.read().await.capture_dir.clone();
    let files = tokio::task::spawn_blocking(move || {
        let mut out = Vec::new();
        collect_ts(&root, &root, 0, &mut out);
        out.sort_by(|a, b| b.2.cmp(&a.2));
        out.into_iter()
            .map(|(rel, size, mtime, sidecar)| {
                json!({ "file": rel, "size": size, "mtime": mtime, "sidecar": sidecar })
            })
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();
    Json(json!({ "ok": true, "files": files })).into_response()
}

fn collect_ts(
    root: &std::path::Path,
    dir: &std::path::Path,
    depth: usize,
    out: &mut Vec<(String, u64, u64, bool)>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth < 3 {
                collect_ts(root, &path, depth + 1, out);
            }
        } else if path.extension().is_some_and(|e| e == "ts") {
            let meta = entry.metadata().ok();
            let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            let mtime = meta
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let sidecar = analyze::services_json_path_for_ts(&path).is_file();
            if let Ok(rel) = path.strip_prefix(root) {
                out.push((rel.to_string_lossy().into_owned(), size, mtime, sidecar));
            }
        }
    }
}

/// Playout form values derived from a capture's `_services.json` sidecar.
async fn api_playout_suggest(State(state): State<AppState>, Query(q): Query<FileQuery>) -> Response {
    let path = state.config.read().await.resolve_output(&q.file);
    let sidecar = analyze::services_json_path_for_ts(&path);
    match CaptureServicesRecord::read_file(&sidecar)
        .and_then(|rec| ReferenceCapture::from_tune(q.file.clone(), &rec.tune).map(|f| (rec, f)))
    {
        Ok((rec, form)) => Json(json!({ "ok": true, "form": form, "tune": rec.tune })).into_response(),
        Err(e) => error_json(StatusCode::NOT_FOUND, &format!("no usable sidecar: {e:#}")),
    }
}

// ---- satellites / reference databases --------------------------------------

async fn api_satellites_get(State(state): State<AppState>) -> Response {
    Json(state.satellites.read().await.clone()).into_response()
}

async fn api_satellites_upsert(State(state): State<AppState>, Json(sat): Json<Satellite>) -> Response {
    if sat.name.trim().is_empty() {
        return bad_request("satellite name is required");
    }
    let db = {
        let mut db = state.satellites.write().await;
        db.upsert(sat);
        db.clone()
    };
    persist("satellites", move || db.save());
    ok()
}

async fn api_satellites_delete(State(state): State<AppState>, AxumPath(name): AxumPath<String>) -> Response {
    let (removed, db) = {
        let mut db = state.satellites.write().await;
        (db.remove(&name), db.clone())
    };
    if !removed {
        return error_json(StatusCode::NOT_FOUND, "satellite not found");
    }
    persist("satellites", move || db.save());
    ok()
}

#[derive(Deserialize)]
struct ImportBody {
    /// KingOfSat position code, e.g. "42E", "13E", "19.2E".
    pos: String,
    name: Option<String>,
    longitude: Option<String>,
}

async fn api_satellites_import(State(state): State<AppState>, Json(req): Json<ImportBody>) -> Response {
    let pos = req.pos.trim().to_string();
    if pos.is_empty() {
        return bad_request("pos is required (e.g. 42E)");
    }
    let fetch_pos = pos.clone();
    let (label, tps) =
        match tokio::task::spawn_blocking(move || crate::satellites::fetch_position(&fetch_pos)).await {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => return error_json(StatusCode::BAD_GATEWAY, &e.to_string()),
            Err(e) => return error_json(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        };
    let name = req.name.unwrap_or(label);
    let count = tps.len();
    let db = {
        let mut db = state.satellites.write().await;
        // Keep the user's LNB / DiSEqC settings when refreshing a known satellite.
        let (lnb, diseqc) = db
            .index_of(&name)
            .map(|i| (db.satellites[i].lnb.clone(), db.satellites[i].diseqc))
            .unwrap_or(("universal".into(), 0));
        db.upsert(Satellite {
            name: name.clone(),
            longitude: req.longitude.unwrap_or_else(|| pos.clone()),
            lnb,
            diseqc,
            transponders: tps,
        });
        db.clone()
    };
    persist("satellites", move || db.save());
    Json(json!({ "ok": true, "name": name, "transponders": count })).into_response()
}

async fn api_ref_tps_get(State(state): State<AppState>) -> Response {
    Json(state.ref_tps.read().await.clone()).into_response()
}

async fn api_ref_tps_add(State(state): State<AppState>, Json(tp): Json<SatTransponder>) -> Response {
    if tp.frequency_mhz == 0 || tp.symbol_rate_ksps == 0 {
        return bad_request("frequency and symbol rate are required");
    }
    let db = {
        let mut db = state.ref_tps.write().await;
        db.add(tp);
        db.clone()
    };
    let count = db.transponders.len();
    persist("reference transponders", move || db.save());
    Json(json!({ "ok": true, "count": count })).into_response()
}

async fn api_ref_tps_delete(State(state): State<AppState>, AxumPath(index): AxumPath<usize>) -> Response {
    let (removed, db) = {
        let mut db = state.ref_tps.write().await;
        (db.remove(index), db.clone())
    };
    if !removed {
        return error_json(StatusCode::NOT_FOUND, "transponder index out of range");
    }
    persist("reference transponders", move || db.save());
    ok()
}

async fn api_ref_caps_get(State(state): State<AppState>) -> Response {
    Json(state.ref_caps.read().await.clone()).into_response()
}

async fn api_ref_caps_upsert(State(state): State<AppState>, Json(cap): Json<ReferenceCapture>) -> Response {
    if cap.name.trim().is_empty() || cap.file.trim().is_empty() {
        return bad_request("name and file are required");
    }
    let db = {
        let mut db = state.ref_caps.write().await;
        db.upsert(cap);
        db.clone()
    };
    persist("reference captures", move || db.save());
    ok()
}

async fn api_ref_caps_delete(State(state): State<AppState>, AxumPath(name): AxumPath<String>) -> Response {
    let (removed, db) = {
        let mut db = state.ref_caps.write().await;
        (db.remove(&name), db.clone())
    };
    if !removed {
        return error_json(StatusCode::NOT_FOUND, "capture not found");
    }
    persist("reference captures", move || db.save());
    ok()
}

// ---- channels ---------------------------------------------------------------

async fn api_channels_get(State(state): State<AppState>) -> Response {
    Json(state.channels.read().await.clone()).into_response()
}

async fn api_channels_delete(State(state): State<AppState>, AxumPath(id): AxumPath<u64>) -> Response {
    let (removed, db) = {
        let mut db = state.channels.write().await;
        (db.remove(id), db.clone())
    };
    if !removed {
        return error_json(StatusCode::NOT_FOUND, "channel not found");
    }
    persist("channels", move || db.save());
    ok()
}

#[derive(Deserialize)]
struct ClearQuery {
    delivery: String,
    /// Only channels of this satellite (satellite delivery); "-" = channels without one.
    satellite: Option<String>,
}

async fn api_channels_clear(State(state): State<AppState>, Query(q): Query<ClearQuery>) -> Response {
    let delivery = match parse_category(&q.delivery) {
        Ok(DeliveryCategory::Satellite) => ChannelDelivery::Satellite,
        Ok(DeliveryCategory::Terrestrial) => ChannelDelivery::Terrestrial,
        Ok(DeliveryCategory::Cable) => ChannelDelivery::Cable,
        Err(e) => return bad_request(e),
    };
    let (removed, db) = {
        let mut db = state.channels.write().await;
        let before = db.channels.len();
        db.channels.retain(|c| {
            let sat_match = match q.satellite.as_deref() {
                None => true,
                Some("-") => c.satellite.is_none(),
                Some(s) => c.satellite.as_deref() == Some(s),
            };
            !(c.delivery == delivery && sat_match)
        });
        (before - db.channels.len(), db.clone())
    };
    persist("channels", move || db.save());
    Json(json!({ "ok": true, "removed": removed })).into_response()
}

// ---- session jobs -------------------------------------------------------------

async fn api_status(State(state): State<AppState>) -> Response {
    Json(state.sessions.status().await).into_response()
}

async fn api_tuner_stop(State(state): State<AppState>) -> Response {
    state.sessions.stop_tuner().await;
    ok()
}

/// Tuning fields shared by monitor / manual play.
#[derive(Debug, Default, Deserialize)]
struct TuneSpec {
    delivery: String,
    device_name: Option<String>,
    /// Catalogue satellite: supplies LNB / DiSEqC unless overridden.
    satellite: Option<String>,
    frequency_mhz: u32,
    delivery_system: Option<String>,
    polarity: Option<String>,
    symbol_rate_ksps: Option<u32>,
    modulation: Option<String>,
    fec: Option<String>,
    lnb: Option<String>,
    diseqc: Option<u32>,
    bandwidth_mhz: Option<u8>,
    plp: Option<u8>,
}

/// Builds tuning parameters (with tuner, LNB and DiSEqC resolved) from a spec.
async fn tune_params(state: &AppState, t: &TuneSpec) -> anyhow::Result<CaptureParams> {
    let cat = parse_category(&t.delivery)?;
    anyhow::ensure!(t.frequency_mhz > 0, "frequency_mhz is required");
    let (device, _) = resolve_tuner(state, cat, t.device_name.as_deref(), None)?;
    let sat = match &t.satellite {
        Some(name) => {
            let db = state.satellites.read().await;
            db.index_of(name).map(|i| db.satellites[i].clone())
        }
        None => None,
    };
    let is_sat = cat == DeliveryCategory::Satellite;
    Ok(CaptureParams {
        device_name: Some(device),
        adapter: None,
        delivery_system: t.delivery_system.clone().unwrap_or_else(|| {
            match cat {
                DeliveryCategory::Satellite => "DVB-S2",
                DeliveryCategory::Terrestrial => "DVB-T2",
                DeliveryCategory::Cable => "DVB-C",
            }
            .into()
        }),
        frequency_hz: u64::from(t.frequency_mhz) * 1_000_000,
        polarity: t.polarity.clone().filter(|_| is_sat),
        symbol_rate: t.symbol_rate_ksps.map(|s| u64::from(s) * 1_000),
        modulation: t.modulation.clone(),
        fec: t.fec.clone().filter(|_| is_sat),
        lnb: is_sat.then(|| {
            t.lnb
                .as_deref()
                .map(tsp_lnb_value)
                .or_else(|| sat.as_ref().map(|s| s.tsp_lnb()))
                .unwrap_or_else(|| "Extended".into())
        }),
        satellite_number: is_sat.then(|| t.diseqc.or(sat.as_ref().map(|s| s.diseqc)).unwrap_or(0)),
        bandwidth_hz: t
            .bandwidth_mhz
            .filter(|_| cat == DeliveryCategory::Terrestrial)
            .map(|b| u64::from(b) * 1_000_000),
        guard_interval: None,
        plp: t.plp,
        dvb_status_interval_s: None,
        signal_timeout_s: 10,
        seconds: 0,
        output: String::new(),
        live_analyze: false,
    })
}

async fn api_monitor(State(state): State<AppState>, Json(t): Json<TuneSpec>) -> Response {
    let params = match tune_params(&state, &t).await {
        Ok(p) => p,
        Err(e) => return bad_request(e),
    };
    match state.sessions.start_monitor(params).await {
        Ok(()) => ok(),
        Err(e) => bad_request(e),
    }
}

/// Body of `POST /api/session/capture`.
#[derive(Debug, Deserialize)]
struct CaptureBody {
    delivery: String,
    device_name: Option<String>,
    /// Seconds per target (default: the configured capture length).
    seconds: Option<u32>,
    /// Satellite: catalogue name, or the file-name prefix for a manual TP.
    satellite: Option<String>,
    /// Satellite TPs to record; omitted = every TP of the catalogue satellite.
    tps: Option<Vec<SatTransponder>>,
    lnb: Option<String>,
    diseqc: Option<u32>,
    // Terrestrial / cable mux.
    frequency_mhz: Option<u32>,
    delivery_system: Option<String>,
    bandwidth_mhz: Option<u8>,
    modulation: Option<String>,
    symbol_rate_ksps: Option<u32>,
    plp: Option<u8>,
    guard_interval: Option<String>,
}

async fn api_capture(State(state): State<AppState>, Json(b): Json<CaptureBody>) -> Response {
    match capture_job(&state, b).await {
        Ok(job) => match state.sessions.start_capture(job).await {
            Ok(()) => ok(),
            Err(e) => bad_request(e),
        },
        Err(e) => bad_request(e),
    }
}

async fn capture_job(state: &AppState, b: CaptureBody) -> anyhow::Result<CaptureJob> {
    let cat = parse_category(&b.delivery)?;
    let (device, _) = resolve_tuner(state, cat, b.device_name.as_deref(), None)?;
    let seconds = match b.seconds {
        Some(s) if s > 0 => s,
        _ => state.config.read().await.capture_seconds,
    };
    match cat {
        DeliveryCategory::Satellite => {
            let name = b.satellite.clone().unwrap_or_else(|| "manual".into());
            let known = {
                let db = state.satellites.read().await;
                db.index_of(&name).map(|i| db.satellites[i].clone())
            };
            let mut sat = known.unwrap_or(Satellite {
                name,
                longitude: String::new(),
                lnb: "universal".into(),
                diseqc: 0,
                transponders: Vec::new(),
            });
            if let Some(tps) = b.tps {
                sat.transponders = tps;
            }
            if let Some(l) = b.lnb.filter(|l| !l.is_empty()) {
                sat.lnb = l;
            }
            if let Some(d) = b.diseqc {
                sat.diseqc = d;
            }
            anyhow::ensure!(!sat.transponders.is_empty(), "no transponders to capture");
            Ok(CaptureJob::Satellite { satellite: sat, device, seconds })
        }
        DeliveryCategory::Terrestrial => {
            let mhz = b.frequency_mhz.ok_or_else(|| anyhow::anyhow!("frequency_mhz is required"))?;
            Ok(CaptureJob::Terrestrial {
                mux: TerrestrialMux {
                    label: format!("{mhz} MHz"),
                    location: String::new(),
                    uhf_channel: (21..=69u8)
                        .find(|&c| channel_scan::uhf_channel_mhz(c) == Some(mhz)),
                    frequency_mhz: mhz,
                    delivery_system: b.delivery_system.unwrap_or_else(|| "DVB-T2".into()),
                    bandwidth_mhz: b.bandwidth_mhz.unwrap_or(8),
                    guard_interval: b.guard_interval.unwrap_or_else(|| "auto".into()),
                    modulation: b.modulation.filter(|m| !m.is_empty()),
                    fec: None,
                    plp: b.plp,
                    source: "web".into(),
                },
                device,
                seconds,
            })
        }
        DeliveryCategory::Cable => {
            let mhz = b.frequency_mhz.ok_or_else(|| anyhow::anyhow!("frequency_mhz is required"))?;
            let sr = b.symbol_rate_ksps.unwrap_or(6875);
            Ok(CaptureJob::Cable {
                mux: CableMux {
                    label: format!("{mhz} MHz {sr}"),
                    location: String::new(),
                    frequency_mhz: mhz,
                    symbol_rate_ksps: sr,
                    delivery_system: b.delivery_system.unwrap_or_else(|| "DVB-C".into()),
                    modulation: b.modulation.unwrap_or_else(|| "256-QAM".into()),
                    fec: None,
                    source: "web".into(),
                },
                device,
                seconds,
            })
        }
    }
}

/// Body of `POST /api/session/scan`.
#[derive(Debug, Deserialize)]
struct ScanBody {
    delivery: String,
    device_name: Option<String>,
    satellite: Option<String>,
    /// Satellite TP filter ("10719v", "10930"); empty = all.
    #[serde(default)]
    tps: Vec<String>,
    /// Terrestrial / cable frequencies (terrestrial: empty = UHF 21-69).
    #[serde(default)]
    frequencies_mhz: Vec<u32>,
    bandwidth_mhz: Option<u8>,
    symbol_rate_ksps: Option<u32>,
    modulation: Option<String>,
    listen_s: Option<u32>,
    #[serde(default = "yes")]
    include_radio: bool,
    #[serde(default)]
    include_scrambled: bool,
    #[serde(default = "yes")]
    save: bool,
    #[serde(default)]
    replace: bool,
}

fn yes() -> bool {
    true
}

async fn api_scan(State(state): State<AppState>, Json(b): Json<ScanBody>) -> Response {
    let job = async {
        let cat = parse_category(&b.delivery)?;
        let (device, model) = resolve_tuner(&state, cat, b.device_name.as_deref(), None)?;
        let targets = match cat {
            DeliveryCategory::Satellite => {
                let name = b.satellite.as_deref().ok_or_else(|| anyhow::anyhow!("satellite is required"))?;
                let db = state.satellites.read().await;
                let sat = db
                    .index_of(name)
                    .map(|i| &db.satellites[i])
                    .ok_or_else(|| anyhow::anyhow!("satellite not found: {name}"))?;
                channel_scan::satellite_targets(sat, &device, Some(&model), &b.tps)
            }
            DeliveryCategory::Terrestrial => channel_scan::terrestrial_targets(
                &device,
                Some(&model),
                &b.frequencies_mhz,
                b.bandwidth_mhz.unwrap_or(8),
            ),
            DeliveryCategory::Cable => {
                anyhow::ensure!(!b.frequencies_mhz.is_empty(), "cable scan needs frequencies_mhz");
                channel_scan::cable_targets(
                    &device,
                    Some(&model),
                    &b.frequencies_mhz,
                    b.symbol_rate_ksps.unwrap_or(6875),
                    b.modulation.as_deref().unwrap_or("256-QAM"),
                )
            }
        };
        anyhow::ensure!(!targets.is_empty(), "nothing to scan (check the TP filter)");
        Ok::<_, anyhow::Error>(ScanJob {
            device,
            targets,
            opts: ScanOptions {
                listen_secs: b.listen_s.unwrap_or(5),
                include_radio: b.include_radio,
                include_scrambled: b.include_scrambled,
                ..Default::default()
            },
            save: b.save,
            replace: b.replace,
        })
    }
    .await;
    match job {
        Ok(job) => match state.sessions.start_scan(job).await {
            Ok(()) => ok(),
            Err(e) => bad_request(e),
        },
        Err(e) => bad_request(e),
    }
}

/// Body of `POST /api/session/play`: a saved channel, or a tune + service id.
#[derive(Debug, Deserialize)]
struct PlayBody {
    channel_id: Option<u64>,
    tune: Option<TuneSpec>,
    service_id: Option<u64>,
    name: Option<String>,
}

async fn api_play(State(state): State<AppState>, Json(b): Json<PlayBody>) -> Response {
    let job = async {
        if let Some(id) = b.channel_id {
            let ch = state
                .channels
                .read()
                .await
                .channels
                .iter()
                .find(|c| c.id == id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("channel {id} not found"))?;
            let cat = match ch.delivery {
                ChannelDelivery::Satellite => DeliveryCategory::Satellite,
                ChannelDelivery::Terrestrial => DeliveryCategory::Terrestrial,
                ChannelDelivery::Cable => DeliveryCategory::Cable,
            };
            let (device, _) = resolve_tuner(&state, cat, None, ch.tuner_model.as_deref())?;
            let params = ch.to_capture_params(device, 10)?;
            return Ok::<_, anyhow::Error>(PlayJob {
                params,
                service_id: ch.service_id,
                channel_id: Some(ch.id),
                name: ch.name,
            });
        }
        let tune = b.tune.as_ref().ok_or_else(|| anyhow::anyhow!("channel_id or tune is required"))?;
        let sid = b.service_id.ok_or_else(|| anyhow::anyhow!("service_id is required"))?;
        Ok(PlayJob {
            params: tune_params(&state, tune).await?,
            service_id: sid,
            channel_id: None,
            name: b.name.clone().unwrap_or_else(|| format!("service {sid}")),
        })
    }
    .await;
    match job {
        Ok(job) => match state.sessions.start_play(job).await {
            Ok(url) => Json(json!({ "ok": true, "hls_url": url })).into_response(),
            Err(e) => bad_request(e),
        },
        Err(e) => bad_request(e),
    }
}

async fn api_playout(State(state): State<AppState>, Json(form): Json<ReferenceCapture>) -> Response {
    let input = state.config.read().await.resolve_output(&form.file);
    if !input.is_file() {
        return bad_request(format!("file not found: {}", input.display()));
    }
    if !discovery(&state)
        .devices
        .iter()
        .any(|d| d.kind == crate::devices::DeviceKind::Modulator)
    {
        return bad_request("no modulator (DTU-315) connected");
    }
    match form.to_play_params(input.to_string_lossy().into_owned()) {
        Ok(params) => match state.sessions.start_playout(params).await {
            Ok(()) => ok(),
            Err(e) => bad_request(e),
        },
        Err(e) => bad_request(e),
    }
}

async fn api_playout_stop(State(state): State<AppState>) -> Response {
    state.sessions.stop_playout().await;
    ok()
}

// ---- HLS + WebSocket -------------------------------------------------------------

async fn hls_file(AxumPath(path): AxumPath<String>) -> Response {
    let rel = std::path::Path::new(path.trim_start_matches('/'));
    if rel
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let file = session::hls_root().join(rel);
    match tokio::fs::read(&file).await {
        Ok(data) => {
            let ct = if path.ends_with(".m3u8") {
                "application/vnd.apple.mpegurl"
            } else if path.ends_with(".ts") {
                "video/mp2t"
            } else {
                "video/mp4"
            };
            ([(header::CONTENT_TYPE, ct), (header::CACHE_CONTROL, "no-store")], data).into_response()
        }
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn ws_upgrade(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

async fn handle_ws(mut socket: WebSocket, state: AppState) {
    let mut rx = state.sessions.subscribe();
    let disc = discovery(&state);
    let hello = [
        WsEvent::Status {
            status: state.sessions.status().await,
        },
        WsEvent::Devices {
            devices: disc.devices,
            warnings: disc.warnings,
        },
    ];
    for ev in &hello {
        if send_event(&mut socket, ev).await.is_err() {
            return;
        }
    }
    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(ev) => {
                    if send_event(&mut socket, &ev).await.is_err() {
                        break;
                    }
                }
                // A slow client missed some events: resync with a fresh status
                // instead of dropping the connection.
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let status = state.sessions.status().await;
                    if send_event(&mut socket, &WsEvent::Status { status }).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(Message::Ping(p))) => {
                    if socket.send(Message::Pong(p)).await.is_err() {
                        break;
                    }
                }
                _ => {}
            },
        }
    }
}

async fn send_event(socket: &mut WebSocket, ev: &WsEvent) -> Result<(), ()> {
    let text = serde_json::to_string(ev).map_err(|_| ())?;
    socket.send(Message::Text(text)).await.map_err(|_| ())
}

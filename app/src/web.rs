//! Local web interface for dvbhub.
//!
//! `dvbhub serve` starts a small HTTP server (axum) on `localhost` that exposes
//! the same engine the CLI uses: device discovery, reference transponders,
//! live tuning with signal/services, capture control, and (later) scan/playback.
//! The UI is a single embedded HTML page driven by a JSON API and WebSocket
//! push updates.

use crate::analyze;
use crate::channels::{ChannelDb, ChannelDelivery, SavedChannel};
use crate::config::{AppConfig, TsduckPaths};
use crate::lastlog;
use crate::devices::Discovery;
use crate::monitor::{self, DeviceState};
use crate::refdb::{ReferenceCapture, ReferenceCaptureDb, ReferenceTpDb};
use crate::satellites::{SatTransponder, Satellite, SatelliteDb};
use crate::session::{
    CaptureStartRequest, PlayStartRequest, PlayoutRequest, SatCapturePlan, SessionManager,
    TuneRequest, WsEvent,
};
use crate::transponders;
use crate::tsduck;
use tokio::sync::RwLock;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path as AxumPath, Query, State,
    },
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use axum::http::{header, StatusCode};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

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

/// The single embedded UI page.
const INDEX_HTML: &str = include_str!("../web/index.html");

/// Bundled hls.js so the UI works fully offline (no CDN dependency).
const HLS_JS: &str = include_str!("../web/hls.min.js");

/// Runs the web server until the process is interrupted. Blocking entry point
/// for the CLI (`dvbhub serve`); builds its own Tokio runtime.
pub fn serve_blocking(bind: &str, port: u16) -> anyhow::Result<()> {
    lastlog::event("cmd.serve", &format!("bind={bind} port={port}"));
    let paths = TsduckPaths::resolve()?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(serve(paths, bind, port))
}

/// Async server bootstrap.
async fn serve(paths: TsduckPaths, bind: &str, port: u16) -> anyhow::Result<()> {
    let paths = Arc::new(paths);
    let cfg = AppConfig::load();
    // Best-effort: ensure the capture directory exists and persist defaults on
    // first run so the user has a concrete path to edit in the UI.
    if tokio::fs::create_dir_all(&cfg.capture_dir).await.is_err() {
        eprintln!(
            "warning: could not create capture dir {}",
            cfg.capture_dir.display()
        );
    }
    let _ = cfg.save();
    println!("config file: {}", crate::config::config_file_path().display());
    println!("capture dir: {}", cfg.capture_dir.display());
    println!("log file:    {}", crate::config::log_file_path().display());
    let config = Arc::new(RwLock::new(cfg));

    let (sessions, _rx) = SessionManager::new(paths.clone(), config.clone());

    // Load persisted databases (seeded from bundled snapshots on first run).
    let satellites = Arc::new(RwLock::new(SatelliteDb::load()));
    let ref_tps = Arc::new(RwLock::new(ReferenceTpDb::load()));
    let ref_caps = Arc::new(RwLock::new(ReferenceCaptureDb::load()));
    let channels = Arc::new(RwLock::new(ChannelDb::load()));

    // Start the hotplug monitor; it keeps `device_state` fresh and pushes
    // WsEvent::Devices on every change.
    let device_state: DeviceState = Arc::new(std::sync::RwLock::new(Discovery {
        devices: Vec::new(),
        warnings: Vec::new(),
    }));
    monitor::spawn(paths.clone(), device_state.clone(), sessions.subscribe_sender());

    let state = AppState {
        paths,
        config,
        sessions,
        device_state,
        satellites,
        ref_tps,
        ref_caps,
        channels,
    };

    let app = Router::new()
        .route("/", get(index))
        .route("/hls.js", get(hls_js))
        .route("/api/health", get(health))
        .route("/api/devices", get(api_devices))
        .route("/api/transponders", get(api_transponders))
        .route("/api/config", get(api_config_get).post(api_config_set))
        .route("/api/analyze", get(api_analyze))
        .route("/api/captures", get(api_captures))
        .route(
            "/api/satellites",
            get(api_satellites_get).post(api_satellites_upsert),
        )
        .route("/api/satellites/import", post(api_satellites_import))
        .route("/api/satellites/:name", axum::routing::delete(api_satellites_delete))
        .route(
            "/api/reference-transponders",
            get(api_ref_tps_get).post(api_ref_tps_add),
        )
        .route(
            "/api/reference-transponders/:index",
            axum::routing::delete(api_ref_tps_delete),
        )
        .route(
            "/api/reference-captures",
            get(api_ref_caps_get).post(api_ref_caps_upsert),
        )
        .route(
            "/api/reference-captures/:name",
            axum::routing::delete(api_ref_caps_delete),
        )
        .route("/api/channels", get(api_channels_get).post(api_channels_save))
        .route(
            "/api/channels/:id",
            axum::routing::delete(api_channels_delete),
        )
        .route("/api/session/status", get(api_session_status))
        .route("/api/session/tune", post(api_session_tune).delete(api_session_tune_stop))
        .route("/api/session/capture", post(api_session_capture).delete(api_session_capture_stop))
        .route(
            "/api/session/capture-sat",
            post(api_session_capture_sat).delete(api_session_capture_sat_stop),
        )
        .route("/api/session/scan", post(api_session_scan).delete(api_session_scan_stop))
        .route("/api/session/play", post(api_session_play).delete(api_session_play_stop))
        .route(
            "/api/session/playout",
            post(api_session_playout).delete(api_session_playout_stop),
        )
        .route("/hls/*path", get(hls_file))
        .route("/ws", get(ws_upgrade))
        .with_state(state);

    let addr = format!("{bind}:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    println!("dvbhub web interface listening on http://{addr}");
    println!("(press Ctrl-C to stop)");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

/// Resolves when the process receives Ctrl-C, for graceful shutdown.
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    println!("\nshutting down dvbhub web interface");
}

/// Serves the embedded UI page.
async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

/// Serves the bundled hls.js library (so the UI needs no internet access).
async fn hls_js() -> Response {
    (
        [(header::CONTENT_TYPE, "application/javascript")],
        HLS_JS,
    )
        .into_response()
}

/// Lightweight health/version probe used by the UI to show connection status.
async fn health(State(state): State<AppState>) -> Response {
    let paths = state.paths.clone();
    let version = tokio::task::spawn_blocking(move || tsduck::version(&paths))
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "unknown".to_string());

    Json(json!({ "ok": true, "tsduck_version": version })).into_response()
}

/// Returns the cached device snapshot maintained by the hotplug monitor.
async fn api_devices(State(state): State<AppState>) -> Response {
    let discovery = state
        .device_state
        .read()
        .map(|d| d.clone())
        .unwrap_or_else(|_| Discovery {
            devices: Vec::new(),
            warnings: Vec::new(),
        });
    Json(discovery).into_response()
}

/// Returns the fixed reference transponder list.
async fn api_transponders() -> Response {
    Json(transponders::REFERENCE_TRANSPONDERS).into_response()
}

/// Returns the current application config (capture directory, etc.).
async fn api_config_get(State(state): State<AppState>) -> Response {
    let cfg = state.config.read().await.clone();
    Json(json!({
        "ok": true,
        "capture_dir": cfg.capture_dir.to_string_lossy(),
        "config_file": crate::config::config_file_path().to_string_lossy(),
        "log_file": crate::config::log_file_path().to_string_lossy(),
    }))
    .into_response()
}

/// Body for updating the application config.
#[derive(Deserialize)]
struct ConfigUpdate {
    capture_dir: String,
}

/// Updates the capture directory and persists the config to disk.
async fn api_config_set(
    State(state): State<AppState>,
    Json(req): Json<ConfigUpdate>,
) -> Response {
    let dir = req.capture_dir.trim();
    if dir.is_empty() {
        return error_json(StatusCode::BAD_REQUEST, "capture_dir cannot be empty");
    }
    match state
        .sessions
        .set_capture_dir(std::path::PathBuf::from(dir))
        .await
    {
        Ok(cfg) => Json(json!({
            "ok": true,
            "capture_dir": cfg.capture_dir.to_string_lossy(),
        }))
        .into_response(),
        Err(e) => error_json(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

/// Query parameters for `/api/analyze`.
#[derive(Deserialize)]
struct AnalyzeQuery {
    file: String,
}

/// Analyzes a transport-stream file (TS id, bitrate, service list).
async fn api_analyze(
    State(state): State<AppState>,
    query: Result<Query<AnalyzeQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Query(q) = match query {
        Ok(q) => q,
        Err(_) => return error_json(StatusCode::BAD_REQUEST, "missing ?file=<path>"),
    };
    let paths = state.paths.clone();
    let file = q.file.clone();
    match tokio::task::spawn_blocking(move || analyze::analyze_file(&paths, &file)).await {
        Ok(Ok(summary)) => Json(summary).into_response(),
        Ok(Err(e)) => error_json(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        Err(e) => error_json(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn api_session_status(State(state): State<AppState>) -> Response {
    Json(state.sessions.status().await).into_response()
}

async fn api_session_tune(
    State(state): State<AppState>,
    Json(req): Json<TuneRequest>,
) -> Response {
    match state.sessions.clone().start_tune(req).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => error_json(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

async fn api_session_tune_stop(State(state): State<AppState>) -> Response {
    state.sessions.stop_tune().await;
    Json(json!({ "ok": true })).into_response()
}

async fn api_session_capture(
    State(state): State<AppState>,
    Json(req): Json<CaptureStartRequest>,
) -> Response {
    match state.sessions.clone().start_capture(req).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => error_json(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

async fn api_session_capture_stop(State(state): State<AppState>) -> Response {
    let path = state.sessions.stop_capture().await;
    Json(json!({ "ok": true, "output": path })).into_response()
}

async fn api_session_scan(
    State(state): State<AppState>,
    Json(req): Json<TuneRequest>,
) -> Response {
    match state.sessions.clone().start_nit_scan(req).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => error_json(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

async fn api_session_scan_stop(State(state): State<AppState>) -> Response {
    state.sessions.stop_scan().await;
    Json(json!({ "ok": true })).into_response()
}

async fn api_session_play(
    State(state): State<AppState>,
    Json(req): Json<PlayStartRequest>,
) -> Response {
    match state.sessions.clone().start_play(req).await {
        Ok(hls_url) => Json(json!({ "ok": true, "hls_url": hls_url })).into_response(),
        Err(e) => error_json(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

async fn api_session_play_stop(State(state): State<AppState>) -> Response {
    state.sessions.stop_play().await;
    Json(json!({ "ok": true })).into_response()
}

// ---- Satellite database -------------------------------------------------

/// Returns the satellite catalogue.
async fn api_satellites_get(State(state): State<AppState>) -> Response {
    let db = state.satellites.read().await.clone();
    Json(db).into_response()
}

/// Adds or replaces a satellite (matched by name), then persists.
async fn api_satellites_upsert(
    State(state): State<AppState>,
    Json(sat): Json<Satellite>,
) -> Response {
    if sat.name.trim().is_empty() {
        return error_json(StatusCode::BAD_REQUEST, "satellite name is required");
    }
    let db = {
        let mut db = state.satellites.write().await;
        db.upsert(sat);
        db.clone()
    };
    persist_satellites(&db);
    Json(json!({ "ok": true })).into_response()
}

/// Deletes a satellite by name.
async fn api_satellites_delete(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
) -> Response {
    let (removed, db) = {
        let mut db = state.satellites.write().await;
        let removed = db.remove(&name);
        (removed, db.clone())
    };
    if removed {
        persist_satellites(&db);
        Json(json!({ "ok": true })).into_response()
    } else {
        error_json(StatusCode::NOT_FOUND, "satellite not found")
    }
}

/// Body for a KingOfSat import request.
#[derive(Deserialize)]
struct ImportBody {
    /// KingOfSat position code, e.g. "42E", "13E", "19.2E".
    pos: String,
    /// Optional satellite name to store (defaults to the KingOfSat label).
    name: Option<String>,
    /// Optional longitude label (defaults to `pos`).
    longitude: Option<String>,
}

/// Imports a satellite live from KingOfSat and stores it in the catalogue.
async fn api_satellites_import(
    State(state): State<AppState>,
    Json(req): Json<ImportBody>,
) -> Response {
    let pos = req.pos.trim().to_string();
    if pos.is_empty() {
        return error_json(StatusCode::BAD_REQUEST, "pos is required (e.g. 42E)");
    }
    let pos_for_fetch = pos.clone();
    let fetched =
        tokio::task::spawn_blocking(move || crate::satellites::fetch_position(&pos_for_fetch))
            .await;
    let (label, tps) = match fetched {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return error_json(StatusCode::BAD_GATEWAY, &e.to_string()),
        Err(e) => return error_json(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let name = req.name.unwrap_or_else(|| label.clone());
    let longitude = req.longitude.unwrap_or_else(|| pos.clone());
    let count = tps.len();
    let sat = Satellite {
        name: name.clone(),
        longitude,
        lnb: "universal".to_string(),
        diseqc: 0,
        transponders: tps,
    };
    let db = {
        let mut db = state.satellites.write().await;
        // Preserve LNB/DiSEqC if the satellite already exists.
        if let Some(i) = db.index_of(&name) {
            let mut sat = sat;
            sat.lnb = db.satellites[i].lnb.clone();
            sat.diseqc = db.satellites[i].diseqc;
            db.satellites[i] = sat;
        } else {
            db.upsert(sat);
        }
        db.clone()
    };
    persist_satellites(&db);
    Json(json!({ "ok": true, "name": name, "transponders": count })).into_response()
}

/// Persists the satellite DB off-thread (best effort).
fn persist_satellites(db: &SatelliteDb) {
    let db = db.clone();
    tokio::task::spawn_blocking(move || {
        let _ = db.save();
    });
}

// ---- Reference transponders --------------------------------------------

async fn api_ref_tps_get(State(state): State<AppState>) -> Response {
    let db = state.ref_tps.read().await.clone();
    Json(db).into_response()
}

async fn api_ref_tps_add(
    State(state): State<AppState>,
    Json(tp): Json<SatTransponder>,
) -> Response {
    let db = {
        let mut db = state.ref_tps.write().await;
        db.add(tp);
        db.clone()
    };
    let to_save = db.clone();
    tokio::task::spawn_blocking(move || {
        let _ = to_save.save();
    });
    Json(json!({ "ok": true, "count": db.transponders.len() })).into_response()
}

async fn api_ref_tps_delete(
    State(state): State<AppState>,
    AxumPath(index): AxumPath<usize>,
) -> Response {
    let (ok, db) = {
        let mut db = state.ref_tps.write().await;
        let ok = db.remove(index);
        (ok, db.clone())
    };
    if ok {
        let to_save = db.clone();
        tokio::task::spawn_blocking(move || {
            let _ = to_save.save();
        });
        Json(json!({ "ok": true })).into_response()
    } else {
        error_json(StatusCode::NOT_FOUND, "transponder index out of range")
    }
}

// ---- Reference captures -------------------------------------------------

async fn api_ref_caps_get(State(state): State<AppState>) -> Response {
    let db = state.ref_caps.read().await.clone();
    Json(db).into_response()
}

async fn api_ref_caps_upsert(
    State(state): State<AppState>,
    Json(cap): Json<ReferenceCapture>,
) -> Response {
    if cap.name.trim().is_empty() {
        return error_json(StatusCode::BAD_REQUEST, "capture name is required");
    }
    let db = {
        let mut db = state.ref_caps.write().await;
        db.upsert(cap);
        db.clone()
    };
    let to_save = db.clone();
    tokio::task::spawn_blocking(move || {
        let _ = to_save.save();
    });
    Json(json!({ "ok": true })).into_response()
}

async fn api_ref_caps_delete(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
) -> Response {
    let (ok, db) = {
        let mut db = state.ref_caps.write().await;
        let ok = db.remove(&name);
        (ok, db.clone())
    };
    if ok {
        let to_save = db.clone();
        tokio::task::spawn_blocking(move || {
            let _ = to_save.save();
        });
        Json(json!({ "ok": true })).into_response()
    } else {
        error_json(StatusCode::NOT_FOUND, "capture not found")
    }
}

// ---- Saved TV channels -------------------------------------------------

async fn api_channels_get(State(state): State<AppState>) -> Response {
    let db = state.channels.read().await.clone();
    Json(db).into_response()
}

#[derive(Deserialize)]
struct SaveChannelsBody {
    delivery: String,
    tune: TuneRequest,
    #[serde(default)]
    replace: bool,
}

fn parse_delivery(s: &str) -> Option<ChannelDelivery> {
    match s.to_ascii_lowercase().as_str() {
        "satellite" | "sat" | "s" => Some(ChannelDelivery::Satellite),
        "terrestrial" | "terr" | "t" => Some(ChannelDelivery::Terrestrial),
        "cable" | "c" => Some(ChannelDelivery::Cable),
        _ => None,
    }
}

/// Saves FTA services from the current session (after tune) into the catalogue.
async fn api_channels_save(
    State(state): State<AppState>,
    Json(req): Json<SaveChannelsBody>,
) -> Response {
    let delivery = match parse_delivery(&req.delivery) {
        Some(d) => d,
        None => return error_json(StatusCode::BAD_REQUEST, "delivery must be satellite, terrestrial, or cable"),
    };

    let services = state.sessions.status().await.services;
    if services.is_empty() {
        return error_json(
            StatusCode::BAD_REQUEST,
            "no services in session — lock a transponder first",
        );
    }

    let mut base_index = {
        let db = state.channels.read().await;
        db.for_delivery(&delivery)
            .iter()
            .map(|c| c.scan_index)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
    };

    if req.replace {
        let mut db = state.channels.write().await;
        db.clear_delivery(&delivery);
        base_index = 0;
    }

    let mut batch = Vec::new();
    for svc in services.iter().filter(|s| !s.scrambled) {
        batch.push(SavedChannel {
            id: 0,
            delivery: delivery.clone(),
            name: svc.name.clone(),
            service_id: svc.id,
            provider: svc.provider.clone(),
            scrambled: svc.scrambled,
            scan_index: base_index,
            device_name: req.tune.device_name.clone(),
            delivery_system: req.tune.delivery_system.clone(),
            frequency_mhz: req.tune.frequency_mhz,
            polarity: req.tune.polarity.clone(),
            symbol_rate_ksps: req.tune.symbol_rate_ksps,
            modulation: req.tune.modulation.clone(),
            fec: req.tune.fec.clone(),
            satellite_number: req.tune.satellite_number,
            bandwidth_mhz: None,
            guard_interval: None,
            plp: None,
        });
        base_index += 1;
    }

    if batch.is_empty() {
        return error_json(StatusCode::BAD_REQUEST, "no FTA services to save");
    }

    let (added, db) = {
        let mut db = state.channels.write().await;
        let added = db.add_batch(batch);
        (added, db.clone())
    };
    persist_channels(&db);
    Json(json!({ "ok": true, "added": added, "total": db.channels.len() })).into_response()
}

async fn api_channels_delete(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<u64>,
) -> Response {
    let (ok, db) = {
        let mut db = state.channels.write().await;
        let ok = db.remove(id);
        (ok, db.clone())
    };
    if ok {
        persist_channels(&db);
        Json(json!({ "ok": true })).into_response()
    } else {
        error_json(StatusCode::NOT_FOUND, "channel not found")
    }
}

fn persist_channels(db: &ChannelDb) {
    let db = db.clone();
    tokio::task::spawn_blocking(move || {
        let _ = db.save();
    });
}

/// Lists transport-stream files available under the capture directory.
async fn api_captures(State(state): State<AppState>) -> Response {
    let dir = state.config.read().await.capture_dir.clone();
    let mut files = Vec::new();
    collect_ts_files(&dir, &dir, &mut files, 0).await;
    files.sort();
    Json(json!({ "ok": true, "files": files })).into_response()
}

/// Recursively collects `.ts` files (up to two directory levels) as paths
/// relative to `root`.
fn collect_ts_files<'a>(
    root: &'a std::path::Path,
    dir: &'a std::path::Path,
    out: &'a mut Vec<String>,
    depth: usize,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
        let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
            return;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.is_dir() {
                if depth < 1 && path.file_name().and_then(|n| n.to_str()) != Some("hls") {
                    collect_ts_files(root, &path, out, depth + 1).await;
                }
            } else if path.extension().and_then(|e| e.to_str()) == Some("ts") {
                if let Ok(rel) = path.strip_prefix(root) {
                    out.push(rel.to_string_lossy().into_owned());
                }
            }
        }
    })
}

// ---- Satellite capture & playout ---------------------------------------

/// Body for a sequential satellite capture.
#[derive(Deserialize)]
struct SatCaptureBody {
    device_name: Option<String>,
    adapter: Option<u32>,
    satellite: String,
    seconds: Option<u32>,
    signal_timeout_s: Option<u32>,
}

async fn api_session_capture_sat(
    State(state): State<AppState>,
    Json(req): Json<SatCaptureBody>,
) -> Response {
    let sat = {
        let db = state.satellites.read().await;
        match db.index_of(&req.satellite) {
            Some(i) => db.satellites[i].clone(),
            None => return error_json(StatusCode::NOT_FOUND, "satellite not found"),
        }
    };
    let lnb = if sat.lnb.eq_ignore_ascii_case("universal") {
        None
    } else {
        Some(sat.lnb.clone())
    };
    let plan = SatCapturePlan {
        device_name: req.device_name,
        adapter: req.adapter,
        satellite_name: sat.name.clone(),
        diseqc: sat.diseqc,
        lnb,
        seconds: req.seconds.unwrap_or(10),
        signal_timeout_s: req.signal_timeout_s.unwrap_or(15),
        transponders: sat.transponders,
    };
    match state.sessions.clone().start_sat_capture(plan).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => error_json(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

async fn api_session_capture_sat_stop(State(state): State<AppState>) -> Response {
    state.sessions.clone().stop_sat_capture().await;
    Json(json!({ "ok": true })).into_response()
}

async fn api_session_playout(
    State(state): State<AppState>,
    Json(req): Json<PlayoutRequest>,
) -> Response {
    match state.sessions.clone().start_playout(req).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => error_json(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

async fn api_session_playout_stop(State(state): State<AppState>) -> Response {
    state.sessions.stop_playout().await;
    Json(json!({ "ok": true })).into_response()
}

async fn hls_file(
    State(state): State<AppState>,
    axum::extract::Path(path): axum::extract::Path<String>,
) -> Response {
    // Prevent path traversal; allow nested paths such as s10601/stream.m3u8.
    let rel = std::path::Path::new(path.trim_start_matches('/'));
    if rel
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let hls_dir = state.config.read().await.hls_dir();
    let file_path = hls_dir.join(rel);
    match tokio::fs::read(&file_path).await {
        Ok(data) => {
            let ct = if path.ends_with(".m3u8") {
                "application/vnd.apple.mpegurl"
            } else if path.ends_with(".ts") {
                "video/mp2t"
            } else {
                "application/octet-stream"
            };
            ([(header::CONTENT_TYPE, ct)], data).into_response()
        }
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn ws_upgrade(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

async fn handle_ws(mut socket: WebSocket, state: AppState) {
    let mut rx = state.sessions.subscribe();
    let status = state.sessions.status().await;
    if send_event(&mut socket, &WsEvent::Status { status }).await.is_err() {
        return;
    }
    // Send the current device snapshot so the UI badge populates on connect.
    let snapshot = state
        .device_state
        .read()
        .map(|d| d.clone())
        .unwrap_or_else(|_| Discovery {
            devices: Vec::new(),
            warnings: Vec::new(),
        });
    if send_event(
        &mut socket,
        &WsEvent::Devices {
            devices: snapshot.devices,
            warnings: snapshot.warnings,
        },
    )
    .await
    .is_err()
    {
        return;
    }

    loop {
        tokio::select! {
            ev = rx.recv() => {
                match ev {
                    Ok(ev) => {
                        if send_event(&mut socket, &ev).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(p))) => {
                        if socket.send(Message::Pong(p)).await.is_err() {
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

async fn send_event(socket: &mut WebSocket, ev: &WsEvent) -> Result<(), ()> {
    let text = serde_json::to_string(ev).map_err(|_| ())?;
    socket.send(Message::Text(text)).await.map_err(|_| ())
}

/// Builds a JSON error response with the given status.
fn error_json(status: StatusCode, message: &str) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        json!({ "ok": false, "error": message }).to_string(),
    )
        .into_response()
}

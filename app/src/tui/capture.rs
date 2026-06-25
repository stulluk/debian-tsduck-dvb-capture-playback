//! Interactive capture wizard and three-panel SAT capture view.

use crate::config::{AppConfig, TsduckPaths};
use crate::devices::{DeviceKind, DvbDevice};
use crate::analyze::services_display_lines;
use crate::progress::CaptureEvent;
use crate::sat_capture::{self, satellite_slug};
use crate::satellites::{Satellite, SatelliteDb};
use crate::signal::SignalStatus;
use crate::tui::confirm::ConfirmPrompt;
use crate::tui::manual_capture::{draw_manual_capture, ManualCaptureForm};
use crate::tui::settings::CaptureSettingsForm;
use crate::tui::widgets::{
    capture_run_layout, list_menu, list_viewport_rows, log_panel, paragraph, wizard_footer_rows,
    wizard_list_layout,
};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::prelude::*;
use std::io::stdout;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::thread;
use std::time::Duration;

/// Wizard screen identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Device,
    Delivery,
    Satellite,
    Mode,
    TpPick,
    ManualCapture,
    Run,
    Settings,
}

/// First row on the satellite picker — custom capture without catalogue entry.
const MANUAL_SAT_LABEL: &str = "MANUAL SETUP — custom satellite & transponder";

fn sat_list_len(db: &SatelliteDb) -> usize {
    1 + db.satellites.len()
}

fn sat_db_index(sel_sat: usize) -> Option<usize> {
    if sel_sat == 0 {
        None
    } else {
        Some(sel_sat - 1)
    }
}

fn catalogue_sat<'a>(db: &'a SatelliteDb, sel_sat: usize) -> Option<&'a Satellite> {
    sat_db_index(sel_sat).map(|i| &db.satellites[i])
}

/// Full satellite vs single transponder capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureMode {
    FullSat,
    SingleTp,
}

/// Background worker → UI messages during capture.
enum WorkerMsg {
    TpStart { index: usize, total: usize, label: String },
    TpDone { index: usize, locked: bool, services: usize },
    Progress(CaptureEvent),
    Finished { ok: usize, fail: usize },
    Error(String),
}

/// Runs the capture TUI (`dvbhub capture --tui`).
pub fn run_capture_tui(paths: TsduckPaths, auto: bool) -> anyhow::Result<()> {
    let discovery = crate::devices::discover(&paths)?;
    let tuners: Vec<DvbDevice> = discovery
        .devices
        .into_iter()
        .filter(|d| d.kind == DeviceKind::Tuner)
        .filter(|d| d.categories.iter().any(|c| c == "satellite"))
        .collect();
    if tuners.is_empty() {
        anyhow::bail!("no satellite tuner found — connect a DVB-S/S2 device");
    }

    let db = SatelliteDb::load();
    if db.satellites.is_empty() {
        anyhow::bail!(
            "satellite catalogue empty — run: dvbhub update-satellites-json"
        );
    }

    let mut cfg = AppConfig::load();
    let mut sel_device = 0usize;
    let mut sel_sat = 0usize;
    let mut sel_tp = 0usize;
    let mut sel_mode = 0usize; // 0 full, 1 single
    let mut screen = Screen::Device;
    let mut settings_return = Screen::Device;
    let mut settings_form = CaptureSettingsForm::from_config(&cfg);
    let mut full_sat = CaptureMode::FullSat;
    let signal_timeout = 15u32;
    let auto_seconds = 5u32;

    if auto {
        sel_device = 0;
        let db_idx = db
            .satellites
            .iter()
            .position(|s| s.name.eq_ignore_ascii_case("Turksat 42E"))
            .unwrap_or(0);
        sel_sat = db_idx + 1;
        full_sat = CaptureMode::SingleTp;
        sel_tp = db.satellites[db_idx]
            .transponders
            .iter()
            .position(|t| t.frequency_mhz == 11054)
            .unwrap_or(0);
        screen = Screen::Run;
    }

    let mut worker_rx: Option<mpsc::Receiver<WorkerMsg>> = None;
    let mut worker_cancel: Option<Arc<AtomicBool>> = None;
    let mut run_return_screen = Screen::Mode;
    let mut stop_requested = false;
    let mut confirm = ConfirmPrompt::default();
    let mut manual_form = ManualCaptureForm::new_default();
    let mut manual_status = String::new();
    let mut run_state = RunState::default();
    let mut summary_lines = String::new();

    if auto {
        start_worker(
            &paths,
            &cfg,
            &tuners[sel_device],
            catalogue_sat(&db, sel_sat).expect("auto requires catalogue satellite"),
            full_sat,
            sel_tp,
            if auto { auto_seconds } else { cfg.capture_seconds },
            signal_timeout,
            auto,
            &mut run_state,
            &mut worker_rx,
            &mut worker_cancel,
        )?;
    }

    let mut stdout = stdout();
    enable_raw_mode()?;
    stdout.execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut term = Terminal::new(backend)?;

    let result = 'main: loop {
        term.draw(|f| {
            match screen {
                Screen::Device => {
                    let labels: Vec<String> = tuners
                        .iter()
                        .map(|d| format!("{} — {}", d.address, d.label))
                        .collect();
                    let layout = wizard_list_layout(f.area(), false);
                    list_menu(f, layout.list, "Capture — select tuner", &labels, sel_device);
                    paragraph(
                        f,
                        layout.keys,
                        "Keys",
                        "↑↓ PgUp/PgDn · Enter · Esc back · s settings · q quit",
                    );
                }
                Screen::Delivery => {
                    list_menu(
                        f,
                        f.area(),
                        "Delivery system",
                        &[
                            "Satellite (DVB-S/S2)".into(),
                            "Terrestrial — coming soon".into(),
                            "Cable — coming soon".into(),
                        ],
                        0,
                    );
                }
                Screen::Satellite => {
                    let mut labels = vec![MANUAL_SAT_LABEL.into()];
                    labels.extend(db.satellites.iter().map(|s| {
                        format!(
                            "{} — {} ({} TPs, DiSEqC {})",
                            s.name,
                            s.longitude,
                            s.transponders.len(),
                            s.diseqc
                        )
                    }));
                    let layout = wizard_list_layout(f.area(), false);
                    list_menu(f, layout.list, "Select satellite", &labels, sel_sat);
                    paragraph(
                        f,
                        layout.keys,
                        "Keys",
                        "↑↓ PgUp/PgDn · Enter · Esc back · s settings · q quit",
                    );
                }
                Screen::Mode => {
                    let layout = wizard_list_layout(f.area(), false);
                    list_menu(
                        f,
                        layout.list,
                        "Capture mode",
                        &[
                            "Full satellite — every transponder".into(),
                            "Single transponder".into(),
                        ],
                        sel_mode,
                    );
                    paragraph(
                        f,
                        layout.keys,
                        "Keys",
                        "↑↓ · Enter confirm · Esc back · s settings · q quit",
                    );
                }
                Screen::TpPick => {
                    let show_summary = !summary_lines.is_empty();
                    let layout = wizard_list_layout(f.area(), show_summary);
                    if let Some(sat) = catalogue_sat(&db, sel_sat) {
                        let labels: Vec<String> = sat
                            .transponders
                            .iter()
                            .map(|t| {
                                format!(
                                    "{} MHz {} {} SR {} — {}",
                                    t.frequency_mhz,
                                    t.polarity,
                                    t.delivery_system,
                                    t.symbol_rate_ksps,
                                    t.modulation
                                )
                            })
                            .collect();
                        list_menu(f, layout.list, "Select transponder", &labels, sel_tp);
                    } else {
                        list_menu(
                            f,
                            layout.list,
                            "Select transponder",
                            &["(no catalogue satellite — Esc to go back)".into()],
                            0,
                        );
                    }
                    if let Some(summary_area) = layout.summary {
                        paragraph(f, summary_area, "Last run", &summary_lines);
                    }
                    let settings_hint = format!(
                        "Dir: {} · {} s/TP",
                        cfg.capture_dir.display(),
                        cfg.capture_seconds
                    );
                    paragraph(
                        f,
                        layout.keys,
                        "Keys",
                        &format!(
                            "↑↓ PgUp/PgDn scroll · Enter capture · Esc back · s settings · q quit · {}",
                            settings_hint
                        ),
                    );
                }
                Screen::ManualCapture => {
                    draw_manual_capture(f, &manual_form, &manual_status)
                }
                Screen::Run => draw_run(f, &run_state),
                Screen::Settings => settings_form.draw(f),
            }
            confirm.draw(f);
        })?;

        let mut return_to_tp_list = false;
        let mut auto_exit = false;

        if screen == Screen::Run {
            if let Some(rx) = worker_rx.as_ref() {
                while let Ok(msg) = rx.try_recv() {
                    match msg {
                        WorkerMsg::TpStart {
                            index,
                            total,
                            label,
                        } => {
                            run_state.tp_index = index;
                            run_state.tp_total = total;
                            run_state.current_tp = label.clone();
                            run_state.status = format!("Tuning TP {index}/{total}…");
                            run_state.services.clear();
                            run_state.signal = None;
                            run_state.bitrate = None;
                            run_state.recording_left = None;
                            if index > 0 && index <= run_state.tp_queue.len() {
                                run_state.tp_queue[index - 1] =
                                    format!("▶ {label} — tuning");
                            }
                            push_log(&mut run_state, format!("--- TP {index}/{total}: {label} ---"));
                        }
                        WorkerMsg::TpDone {
                            index,
                            locked,
                            services,
                        } => {
                            let tag = if locked { "LOCK ✓" } else { "NO LOCK ✗" };
                            run_state.done_tps.push(format!(
                                "#{index} {tag} — {services} service(s)"
                            ));
                            if index > 0 && index <= run_state.tp_queue.len() {
                                let label = run_state.tp_labels.get(index - 1).cloned().unwrap_or_default();
                                run_state.tp_queue[index - 1] =
                                    format!("{} {label} — {tag}", if locked { "✓" } else { "✗" });
                            }
                            push_log(
                                &mut run_state,
                                format!("TP {index} finished: {tag}, {services} service(s)"),
                            );
                        }
                        WorkerMsg::Progress(ev) => apply_capture_event(&mut run_state, ev),
                        WorkerMsg::Finished { ok, fail } => {
                            summary_lines = format!(
                                "Last capture: {ok} OK, {fail} failed/skipped · output: {}",
                                run_state.output_root.display()
                            );
                            if auto {
                                auto_exit = true;
                            } else {
                                return_to_tp_list = true;
                            }
                        }
                        WorkerMsg::Error(e) => {
                            run_state.status = e;
                        }
                    }
                }
            }
        }

        if auto_exit {
            break 'main Ok(());
        }
        if return_to_tp_list {
            worker_rx = None;
            worker_cancel = None;
            if stop_requested {
                stop_requested = false;
            }
            screen = run_return_screen;
        }

        if event::poll(Duration::from_millis(120))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                let term_h = term.size()?.height;
                let page_rows = |screen: Screen| -> usize {
                    let footer = match screen {
                        Screen::Device | Screen::Satellite | Screen::Mode => wizard_footer_rows(false),
                        Screen::TpPick => wizard_footer_rows(!summary_lines.is_empty()),
                        _ => 0,
                    };
                    list_viewport_rows(term_h, footer)
                };

                if confirm.active {
                    if let Some(yes) = confirm.on_key(key.code) {
                        if yes && screen == Screen::Run && worker_rx.is_some() {
                            stop_requested = true;
                            if let Some(c) = &worker_cancel {
                                c.store(true, Ordering::SeqCst);
                            }
                            run_state.status = "Stopping capture…".into();
                        }
                    }
                    continue;
                }

                if screen == Screen::Settings {
                    if key.code == KeyCode::Esc {
                        screen = settings_return;
                        continue;
                    }
                    if let Some(save) = settings_form.on_key(key) {
                        if save {
                            settings_form.save()?;
                            cfg = AppConfig::load();
                        }
                        screen = settings_return;
                    }
                    continue;
                }

                if screen == Screen::ManualCapture {
                    if key.code == KeyCode::Esc {
                        if manual_form.editing_active() {
                            manual_form.cancel_edit();
                        } else {
                            screen = Screen::Satellite;
                            manual_status.clear();
                        }
                        continue;
                    }
                    if key.code == KeyCode::Char('q') {
                        break 'main Ok(());
                    }
                    if let Some(result) = manual_form.on_key(key.code) {
                        match result {
                            Ok(()) => {
                                let (sat, tp) = manual_form.to_satellite_and_tp();
                                screen = Screen::Run;
                                run_return_screen = Screen::ManualCapture;
                                start_worker_with_tp(
                                    &paths,
                                    &cfg,
                                    &tuners[sel_device],
                                    &sat,
                                    &tp,
                                    if auto { auto_seconds } else { cfg.capture_seconds },
                                    signal_timeout,
                                    &mut run_state,
                                    &mut worker_rx,
                                    &mut worker_cancel,
                                )?;
                                manual_status.clear();
                            }
                            Err(msg) => manual_status = msg.into(),
                        }
                    }
                    continue;
                }

                match key.code {
                    KeyCode::Char('q') if screen != Screen::Settings => {
                        if screen == Screen::Run && worker_rx.is_some() {
                            confirm.ask("Capture durdurulsun mu?");
                        } else {
                            break 'main Ok(());
                        }
                    }
                    KeyCode::Esc => match screen {
                        Screen::Run if worker_rx.is_some() => {
                            confirm.ask("Capture durdurulsun mu?");
                        }
                        Screen::Run => {
                            screen = run_return_screen;
                        }
                        Screen::Delivery => screen = Screen::Device,
                        Screen::Satellite => screen = Screen::Delivery,
                        Screen::Mode => screen = Screen::Satellite,
                        Screen::TpPick => screen = Screen::Mode,
                        Screen::ManualCapture => screen = Screen::Satellite,
                        Screen::Device => break 'main Ok(()),
                        Screen::Settings => {}
                    },
                    KeyCode::Char('s')
                        if matches!(
                            screen,
                            Screen::Device
                                | Screen::Satellite
                                | Screen::Mode
                                | Screen::TpPick
                                | Screen::ManualCapture
                        ) =>
                    {
                        settings_return = screen;
                        settings_form = CaptureSettingsForm::from_config(&cfg);
                        screen = Screen::Settings;
                    }
                    KeyCode::Up => match screen {
                        Screen::Device if sel_device > 0 => sel_device -= 1,
                        Screen::Satellite if sel_sat > 0 => sel_sat -= 1,
                        Screen::Mode if sel_mode > 0 => sel_mode -= 1,
                        Screen::TpPick if sel_tp > 0 => sel_tp -= 1,
                        _ => {}
                    },
                    KeyCode::Down => match screen {
                        Screen::Device if sel_device + 1 < tuners.len() => sel_device += 1,
                        Screen::Satellite if sel_sat + 1 < sat_list_len(&db) => sel_sat += 1,
                        Screen::Mode if sel_mode < 1 => sel_mode += 1,
                        Screen::TpPick => {
                            if let Some(sat) = catalogue_sat(&db, sel_sat) {
                                let n = sat.transponders.len();
                                if sel_tp + 1 < n {
                                    sel_tp += 1;
                                }
                            }
                        }
                        _ => {}
                    },
                    KeyCode::PageUp => {
                        let page = page_rows(screen);
                        match screen {
                            Screen::Device => sel_device = sel_device.saturating_sub(page),
                            Screen::Satellite => sel_sat = sel_sat.saturating_sub(page),
                            Screen::Mode => sel_mode = sel_mode.saturating_sub(page),
                            Screen::TpPick => sel_tp = sel_tp.saturating_sub(page),
                            _ => {}
                        }
                    }
                    KeyCode::PageDown => {
                        let page = page_rows(screen);
                        match screen {
                            Screen::Device => {
                                let max = tuners.len().saturating_sub(1);
                                sel_device = (sel_device + page).min(max);
                            }
                            Screen::Satellite => {
                                let max = sat_list_len(&db).saturating_sub(1);
                                sel_sat = (sel_sat + page).min(max);
                            }
                            Screen::Mode => sel_mode = (sel_mode + page).min(1),
                            Screen::TpPick => {
                                if let Some(sat) = catalogue_sat(&db, sel_sat) {
                                    let max = sat.transponders.len().saturating_sub(1);
                                    sel_tp = (sel_tp + page).min(max);
                                }
                            }
                            _ => {}
                        }
                    }
                    KeyCode::Enter => match screen {
                        Screen::Device => screen = Screen::Delivery,
                        Screen::Delivery => screen = Screen::Satellite,
                        Screen::Satellite => {
                            if sel_sat == 0 {
                                manual_form = ManualCaptureForm::new_default();
                                manual_status.clear();
                                screen = Screen::ManualCapture;
                            } else {
                                screen = Screen::Mode;
                            }
                        }
                        Screen::Mode => {
                            let Some(sat) = catalogue_sat(&db, sel_sat) else {
                                screen = Screen::Satellite;
                                continue;
                            };
                            full_sat = if sel_mode == 0 {
                                CaptureMode::FullSat
                            } else {
                                CaptureMode::SingleTp
                            };
                            screen = if full_sat == CaptureMode::SingleTp {
                                run_return_screen = Screen::TpPick;
                                Screen::TpPick
                            } else {
                                Screen::Run
                            };
                            if screen == Screen::Run {
                                run_return_screen = Screen::Mode;
                                start_worker(
                                    &paths,
                                    &cfg,
                                    &tuners[sel_device],
                                    sat,
                                    full_sat,
                                    sel_tp,
                                    if auto { auto_seconds } else { cfg.capture_seconds },
                                    signal_timeout,
                                    auto,
                                    &mut run_state,
                                    &mut worker_rx,
                                    &mut worker_cancel,
                                )?;
                            }
                        }
                        Screen::TpPick => {
                            let Some(sat) = catalogue_sat(&db, sel_sat) else {
                                screen = Screen::Satellite;
                                continue;
                            };
                            full_sat = CaptureMode::SingleTp;
                            screen = Screen::Run;
                            run_return_screen = Screen::TpPick;
                            start_worker(
                                &paths,
                                &cfg,
                                &tuners[sel_device],
                                sat,
                                full_sat,
                                sel_tp,
                                if auto { auto_seconds } else { cfg.capture_seconds },
                                signal_timeout,
                                auto,
                                &mut run_state,
                                &mut worker_rx,
                                &mut worker_cancel,
                            )?;
                        }
                        Screen::Run => {}
                        Screen::ManualCapture | Screen::Settings => {}
                    },
                    _ => {}
                }
            }
        }

    };

    disable_raw_mode()?;
    term.backend_mut().execute(LeaveAlternateScreen)?;
    term.show_cursor()?;
    result
}

#[derive(Default)]
struct RunState {
    tp_index: usize,
    tp_total: usize,
    current_tp: String,
    status: String,
    device: String,
    tsp_command: String,
    signal: Option<SignalStatus>,
    bitrate: Option<String>,
    recording_left: Option<u32>,
    services: Vec<String>,
    done_tps: Vec<String>,
    tp_queue: Vec<String>,
    tp_labels: Vec<String>,
    logs: Vec<String>,
    output_root: PathBuf,
    capture_sat_name: String,
}

const MAX_LOG_LINES: usize = 200;

fn push_log(st: &mut RunState, line: impl Into<String>) {
    st.logs.push(line.into());
    if st.logs.len() > MAX_LOG_LINES {
        let drop = st.logs.len() - MAX_LOG_LINES;
        st.logs.drain(0..drop);
    }
}

fn parse_bitrate_from_services_header(line: &str) -> Option<String> {
    line.split(',').find_map(|part| {
        let p = part.trim();
        p.strip_suffix(" Mbps")
            .map(|mbps| format!("{mbps} Mbps"))
    })
}

fn apply_capture_event(st: &mut RunState, ev: CaptureEvent) {
    match ev {
        CaptureEvent::Command(cmd) => {
            st.tsp_command = cmd.clone();
            push_log(st, format!("$ {cmd}"));
        }
        CaptureEvent::Status(msg) => {
            st.status = msg.clone();
            push_log(st, msg);
        }
        CaptureEvent::Log(msg) => push_log(st, msg),
        CaptureEvent::Signal(sig) => {
            st.signal = Some(sig.clone());
            if sig.locked {
                st.status = sig.summary();
            }
            push_log(st, sig.summary());
        }
        CaptureEvent::Services(lines) => {
            if let Some(header) = lines.first() {
                st.bitrate = parse_bitrate_from_services_header(header);
            }
            for line in &lines {
                push_log(st, line.clone());
            }
            st.services = lines
                .into_iter()
                .skip(1)
                .map(|l| l.trim_start().to_string())
                .filter(|l| !l.is_empty())
                .collect();
        }
        CaptureEvent::RecordingRemaining(secs) => {
            st.recording_left = Some(secs);
            st.status = format!("Recording… {secs} s remaining");
        }
    }
}

fn draw_run(frame: &mut Frame, st: &RunState) {
    let [left, mid, signal_area, status, log] = capture_run_layout(frame.area());
    let sat_label = if st.capture_sat_name.is_empty() {
        "Capture"
    } else {
        &st.capture_sat_name
    };
    let left_title = format!("{} — TP {}/{}", sat_label, st.tp_index, st.tp_total);
    let tp_lines: Vec<String> = if st.tp_queue.is_empty() {
        vec!["(waiting…)".into()]
    } else {
        st.tp_queue.clone()
    };
    let sel = st.tp_index.saturating_sub(1);
    list_menu(frame, left, &left_title, &tp_lines, sel);

    let svc = if st.services.is_empty() {
        vec!["(waiting for PAT/SDT…)".into()]
    } else {
        st.services.clone()
    };
    list_menu(frame, mid, "Services", &svc, 0);

    let metrics = SignalStatus::metrics_panel(st.signal.as_ref(), st.bitrate.as_deref());
    paragraph(frame, signal_area, "Signal", &metrics);

    let rec = st
        .recording_left
        .map(|s| format!(" · recording {s}s left"))
        .unwrap_or_default();
    let cmd = if st.tsp_command.is_empty() {
        "(tsp command pending…)".to_string()
    } else {
        st.tsp_command.clone()
    };
    paragraph(
        frame,
        status,
        "Status",
        &format!(
            "{}\nDevice: {}\n{}\n\n{}\n\nEsc/q stop · Y/N confirm",
            st.status,
            if st.device.is_empty() {
                "(device…)"
            } else {
                &st.device
            },
            rec,
            cmd,
        ),
    );

    log_panel(frame, log, "Log", &st.logs);
}

fn start_worker(
    paths: &TsduckPaths,
    cfg: &AppConfig,
    device: &DvbDevice,
    sat: &Satellite,
    mode: CaptureMode,
    sel_tp: usize,
    seconds: u32,
    signal_timeout: u32,
    auto: bool,
    run_state: &mut RunState,
    worker_rx: &mut Option<mpsc::Receiver<WorkerMsg>>,
    worker_cancel: &mut Option<Arc<AtomicBool>>,
) -> anyhow::Result<()> {
    let mut tps = sat.transponders.clone();
    if mode == CaptureMode::SingleTp {
        if sel_tp >= tps.len() {
            anyhow::bail!("invalid transponder index");
        }
        tps = vec![tps[sel_tp].clone()];
    }
    if auto {
        tps.truncate(1);
    }
    spawn_capture_worker(
        paths,
        cfg,
        device,
        sat,
        tps,
        seconds,
        signal_timeout,
        run_state,
        worker_rx,
        worker_cancel,
    )
}

fn start_worker_with_tp(
    paths: &TsduckPaths,
    cfg: &AppConfig,
    device: &DvbDevice,
    sat: &Satellite,
    tp: &crate::satellites::SatTransponder,
    seconds: u32,
    signal_timeout: u32,
    run_state: &mut RunState,
    worker_rx: &mut Option<mpsc::Receiver<WorkerMsg>>,
    worker_cancel: &mut Option<Arc<AtomicBool>>,
) -> anyhow::Result<()> {
    spawn_capture_worker(
        paths,
        cfg,
        device,
        sat,
        vec![tp.clone()],
        seconds,
        signal_timeout,
        run_state,
        worker_rx,
        worker_cancel,
    )
}

fn spawn_capture_worker(
    paths: &TsduckPaths,
    cfg: &AppConfig,
    device: &DvbDevice,
    sat: &Satellite,
    tps: Vec<crate::satellites::SatTransponder>,
    seconds: u32,
    signal_timeout: u32,
    run_state: &mut RunState,
    worker_rx: &mut Option<mpsc::Receiver<WorkerMsg>>,
    worker_cancel: &mut Option<Arc<AtomicBool>>,
) -> anyhow::Result<()> {
    let slug = satellite_slug(&sat.name);
    let root = cfg.capture_dir.join(&slug);
    run_state.output_root = root.clone();
    run_state.capture_sat_name = sat.name.clone();
    run_state.status = "Starting capture…".into();
    run_state.device = format!("{} — {}", device.address, device.label);

    run_state.tp_queue = tps
        .iter()
        .map(|tp| {
            format!(
                "○ {} MHz {} {} SR {}",
                tp.frequency_mhz, tp.polarity, tp.delivery_system, tp.symbol_rate_ksps
            )
        })
        .collect();
    run_state.tp_labels = tps
        .iter()
        .map(|tp| {
            format!(
                "{} MHz {} {} SR {}",
                tp.frequency_mhz, tp.polarity, tp.delivery_system, tp.symbol_rate_ksps
            )
        })
        .collect();
    run_state.logs.clear();
    run_state.tsp_command.clear();
    push_log(run_state, format!("Output root: {}", root.display()));
    push_log(run_state, format!("Device: {}", run_state.device));

    let (tx, rx) = mpsc::channel();
    *worker_rx = Some(rx);
    let cancel = Arc::new(AtomicBool::new(false));
    *worker_cancel = Some(cancel.clone());

    let paths = paths.clone();
    let sat = sat.clone();
    let device_name = device.address.clone();
    thread::spawn(move || {
        let total = tps.len();
        let mut ok = 0usize;
        let mut fail = 0usize;
        for (idx, tp) in tps.iter().enumerate() {
            if cancel.load(Ordering::SeqCst) {
                break;
            }
            let label = format!(
                "{} MHz {} {} SR {}",
                tp.frequency_mhz,
                tp.polarity,
                tp.delivery_system,
                tp.symbol_rate_ksps
            );
            let _ = tx.send(WorkerMsg::TpStart {
                index: idx + 1,
                total,
                label: label.clone(),
            });
            let (cap_tx, cap_rx) = mpsc::channel();
            let ui_tx = tx.clone();
            let forward = thread::spawn(move || {
                for ev in cap_rx {
                    let _ = ui_tx.send(WorkerMsg::Progress(ev));
                }
            });
            match sat_capture::capture_transponder(
                &paths,
                &sat,
                &device_name,
                tp,
                seconds,
                signal_timeout,
                &root,
                true,
                Some(cap_tx),
                Some(cancel.clone()),
            ) {
                Ok(out) => {
                    let n = out
                        .run
                        .last_summary
                        .as_ref()
                        .map(|s| s.services.len())
                        .unwrap_or(0);
                    if out.run.success && out.run.ever_locked {
                        ok += 1;
                    } else {
                        fail += 1;
                    }
                    let _ = tx.send(WorkerMsg::TpDone {
                        index: idx + 1,
                        locked: out.run.ever_locked,
                        services: n,
                    });
                    if let Some(summary) = out.run.last_summary.as_ref() {
                        let _ = tx.send(WorkerMsg::Progress(CaptureEvent::Services(
                            services_display_lines(summary),
                        )));
                    }
                }
                Err(e) => {
                    fail += 1;
                    let _ = tx.send(WorkerMsg::Error(e.to_string()));
                }
            }
            let _ = forward.join();
        }
        let _ = tx.send(WorkerMsg::Finished { ok, fail });
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::satellites::SatelliteDb;
    use ratatui::backend::TestBackend;

    #[test]
    fn run_screen_layout_renders() {
        let db = SatelliteDb::seed();
        let sat = db.satellites.first().unwrap().clone();
        let st = RunState {
            tp_index: 1,
            tp_total: 1,
            current_tp: "11054 MHz vertical".into(),
            status: "Recording 5 s…".into(),
            device: "/dev/dvb/adapter0 — PCTV 461".into(),
            tsp_command: "tsp --verbose -I dvb …".into(),
            signal: None,
            bitrate: Some("66.84 Mbps".into()),
            recording_left: Some(3),
            services: vec!["TRT1 HD (10601)".into()],
            done_tps: vec![],
            tp_queue: vec!["▶ 11054 MHz vertical DVB-S2 SR 30000 — tuning".into()],
            tp_labels: vec!["11054 MHz vertical DVB-S2 SR 30000".into()],
            logs: vec![
                "Starting tsp…".into(),
                "LOCKED — strength -45.0 dB, SNR 13.0 dB, BER 0%".into(),
            ],
            output_root: "/tmp".into(),
        };
        let backend = TestBackend::new(100, 30);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| draw_run(f, &st)).unwrap();
        let buf = term.backend().buffer();
        let text = format!("{buf:?}");
        assert!(text.contains("Services"));
        assert!(text.contains("Signal"));
        assert!(text.contains("Bitrate"));
        assert!(text.contains("TRT1"));
        assert!(text.contains("Log"));
        assert!(text.contains("tsp"));
    }
}

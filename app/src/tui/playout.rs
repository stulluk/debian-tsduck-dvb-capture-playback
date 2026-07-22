//! Interactive playout TUI for the DTU-315 modulator.

use crate::analyze::{services_json_path_for_ts, CaptureServicesRecord, TuneRecord};
use crate::bench;
use crate::lastlog;
use crate::config::{AppConfig, TsduckPaths};
use crate::devices::{DeviceKind, DvbDevice};
use crate::pipeline::{is_terrestrial_delivery, play_params_from_tune, PlayParams, PlayoutDelivery};
use crate::tui::confirm::ConfirmPrompt;
use crate::tui::file_picker::FilePicker;
use crate::tui::manual_playout_cable::{
    draw_manual_playout_cable, ManualPlayoutCableForm,
};
use crate::tui::manual_playout_satellite::{
    draw_manual_playout_satellite, ManualPlayoutSatelliteForm,
};
use crate::tui::manual_playout_terrestrial::{
    draw_manual_playout_terrestrial, ManualPlayoutTerrestrialForm,
};
use crate::tui::widgets::{list_menu, log_panel, paragraph, KEYS_FOOTER_HEIGHT};
use crate::tsduck;
use anyhow::Context;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::prelude::*;
use std::io::{stdout, BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeliveryChoice {
    Satellite,
    Terrestrial,
    Cable,
}

impl DeliveryChoice {
    fn label(self) -> &'static str {
        match self {
            Self::Satellite => "Satellite (DVB-S / DVB-S2)",
            Self::Terrestrial => "Terrestrial (DVB-T / DVB-T2)",
            Self::Cable => "Cable (DVB-C)",
        }
    }

    fn from_index(i: usize) -> Self {
        match i {
            0 => Self::Satellite,
            1 => Self::Terrestrial,
            _ => Self::Cable,
        }
    }
}

fn delivery_from_tune(tune: &TuneRecord) -> DeliveryChoice {
    let sys = tune.delivery_system.to_ascii_uppercase();
    if sys.starts_with("DVB-C") {
        DeliveryChoice::Cable
    } else if is_terrestrial_delivery(&sys) {
        DeliveryChoice::Terrestrial
    } else {
        DeliveryChoice::Satellite
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    FilePick,
    FromServices,
    ManualDelivery,
    ManualSatellite,
    ManualTerrestrial,
    ManualCable,
    Run,
}

enum PlayoutMsg {
    Log(String),
}

struct PlayoutRunState {
    status: String,
    summary: String,
    command: String,
    logs: Vec<String>,
    finished: bool,
    success: bool,
}

const MAX_LOG_LINES: usize = 200;

impl PlayoutRunState {
    fn push_log(&mut self, line: String) {
        self.logs.push(line);
        if self.logs.len() > MAX_LOG_LINES {
            let drop = self.logs.len() - MAX_LOG_LINES;
            self.logs.drain(0..drop);
        }
    }
}

/// Runs the playout TUI (`dvbhub playout --tui`).
pub fn run_playout_tui(
    paths: TsduckPaths,
    input: Option<String>,
    auto: bool,
) -> anyhow::Result<()> {
    lastlog::event("tui.playout", &format!("start auto={auto} input={input:?}"));
    let discovery = crate::devices::discover(&paths)?;
    let modulator: Option<DvbDevice> = discovery
        .devices
        .into_iter()
        .find(|d| d.kind == DeviceKind::Modulator);

    if modulator.is_none() {
        return run_no_modulator_screen();
    }

    if auto {
        let ts = input.context("--input FILE.ts is required for --tui-auto")?;
        let path = PathBuf::from(&ts);
        let params = load_playout_from_ts(&path, true)?.unwrap_or_else(|| {
            ManualPlayoutCableForm::new_default().to_play_params(ts)
        });
        return run_playout_tui_auto(paths, params, modulator.as_ref());
    }

    let cfg = AppConfig::load();
    let _ = bench::ensure_bench_sidecars(&cfg.capture_dir);

    let mut screen = Screen::FilePick;
    let mut picker = FilePicker::open(Some(bench::playout_picker_root(&cfg.capture_dir)));
    let mut selected_ts: Option<PathBuf> = input.map(PathBuf::from);
    let mut services: Option<CaptureServicesRecord> = None;
    let mut manual_sat = ManualPlayoutSatelliteForm::new_default();
    let mut manual_terr = ManualPlayoutTerrestrialForm::new_default();
    let mut manual_cable = ManualPlayoutCableForm::new_default();
    let mut delivery_sel = 0usize;
    let mut status = String::new();
    let mut pending_playout: Option<PlayParams> = None;
    let mut run_state = PlayoutRunState {
        status: String::new(),
        summary: String::new(),
        command: String::new(),
        logs: Vec::new(),
        finished: false,
        success: false,
    };
    let mut playout_rx: Option<Receiver<PlayoutMsg>> = None;
    let mut playout_child: Option<Child> = None;
    let mut run_return_screen = Screen::FilePick;
    let mut manual_return = Screen::FilePick;
    let mut confirm = ConfirmPrompt::default();

    if let Some(ref ts) = selected_ts {
        prepare_after_file_pick(ts, &mut screen, &mut services, &mut status)?;
    }

    let mut stdout = stdout();
    enable_raw_mode()?;
    stdout.execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut term = Terminal::new(backend)?;

    let result = 'main: loop {
        if let Some(rx) = playout_rx.as_ref() {
            while let Ok(PlayoutMsg::Log(line)) = rx.try_recv() {
                run_state.push_log(line);
            }
        }
        poll_playout_child(&mut playout_child, &mut run_state);

        term.draw(|f| {
            match screen {
                Screen::FilePick => picker.draw(f),
                Screen::FromServices => draw_from_services(
                    f,
                    modulator.as_ref(),
                    selected_ts.as_ref(),
                    services.as_ref(),
                    &status,
                ),
                Screen::ManualDelivery => draw_manual_delivery(
                    f,
                    delivery_sel,
                    selected_ts.as_ref(),
                    modulator.as_ref(),
                    &status,
                ),
                Screen::ManualSatellite => draw_manual_playout_satellite(
                    f,
                    &manual_sat,
                    selected_ts.as_ref(),
                    modulator.as_ref(),
                    &status,
                ),
                Screen::ManualTerrestrial => draw_manual_playout_terrestrial(
                    f,
                    &manual_terr,
                    selected_ts.as_ref(),
                    modulator.as_ref(),
                    &status,
                ),
                Screen::ManualCable => draw_manual_playout_cable(
                    f,
                    &manual_cable,
                    selected_ts.as_ref(),
                    modulator.as_ref(),
                    &status,
                ),
                Screen::Run => draw_run(f, modulator.as_ref(), &run_state),
            }
            confirm.draw(f);
        })?;

        if let Some(params) = pending_playout.take() {
            begin_playout(
                &paths,
                &params,
                modulator.as_ref(),
                &mut run_state,
                &mut playout_rx,
                &mut playout_child,
            )?;
            screen = Screen::Run;
        }

        if event::poll(Duration::from_millis(150))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                if confirm.active {
                    if let Some(yes) = confirm.on_key(key.code) {
                        if yes && screen == Screen::Run && playout_active(&playout_child, &run_state)
                        {
                            stop_playout(&mut playout_child, &mut run_state);
                            playout_rx = None;
                            screen = run_return_screen;
                        }
                    }
                    continue;
                }

                match key.code {
                    KeyCode::Char('q') => {
                        if screen == Screen::Run && playout_active(&playout_child, &run_state) {
                            confirm.ask("Playout durdurulsun mu?");
                        } else {
                            break 'main Ok(());
                        }
                    }
                    KeyCode::Esc => match screen {
                        Screen::Run if playout_active(&playout_child, &run_state) => {
                            confirm.ask("Playout durdurulsun mu?");
                        }
                        Screen::Run => {
                            screen = run_return_screen;
                        }
                        Screen::FromServices => screen = Screen::FilePick,
                        Screen::ManualDelivery => {
                            screen = manual_return;
                            status.clear();
                        }
                        Screen::ManualSatellite | Screen::ManualTerrestrial | Screen::ManualCable => {
                            if manual_sat.editing_active()
                                || manual_terr.editing_active()
                                || manual_cable.editing_active()
                            {
                                manual_sat.cancel_edit();
                                manual_terr.cancel_edit();
                                manual_cable.cancel_edit();
                            } else {
                                screen = Screen::ManualDelivery;
                                status.clear();
                            }
                        }
                        Screen::FilePick => {
                            let _ = picker.on_key(key);
                        }
                    },
                    _ => {}
                }

                if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
                    continue;
                }

                match screen {
                    Screen::FilePick => {
                        if let Some(ts) = picker.on_key(key) {
                            selected_ts = Some(ts.clone());
                            prepare_after_file_pick(
                                &ts,
                                &mut screen,
                                &mut services,
                                &mut status,
                            )?;
                            delivery_sel = 0;
                        }
                    }
                    Screen::FromServices => match key.code {
                        KeyCode::Enter => {
                            let ts = selected_ts.as_ref().context("missing ts")?;
                            run_return_screen = Screen::FromServices;
                            match load_playout_from_ts(ts, true) {
                                Ok(Some(p)) => {
                                    status.clear();
                                    pending_playout = Some(p);
                                }
                                Ok(None) => {
                                    status =
                                        "sidecar tune could not be mapped — use m for manual"
                                            .into();
                                }
                                Err(e) => status = format!("Error: {e:#}"),
                            }
                        }
                        KeyCode::Char('m') => {
                            manual_return = Screen::FromServices;
                            let tune = services.as_ref().map(|s| &s.tune);
                            open_manual_forms_from_tune(
                                tune,
                                &mut manual_sat,
                                &mut manual_terr,
                                &mut manual_cable,
                                &mut delivery_sel,
                                &mut screen,
                            );
                            status.clear();
                        }
                        _ => {}
                    },
                    Screen::ManualDelivery => match key.code {
                        KeyCode::Up if delivery_sel > 0 => delivery_sel -= 1,
                        KeyCode::Down if delivery_sel < 2 => delivery_sel += 1,
                        KeyCode::Enter => {
                            screen = match DeliveryChoice::from_index(delivery_sel) {
                                DeliveryChoice::Satellite => Screen::ManualSatellite,
                                DeliveryChoice::Terrestrial => Screen::ManualTerrestrial,
                                DeliveryChoice::Cable => Screen::ManualCable,
                            };
                            status.clear();
                        }
                        _ => {}
                    },
                    Screen::ManualSatellite => {
                        if let Some(result) = manual_sat.on_key(key.code) {
                            match result {
                                Ok(()) => {
                                    let ts = selected_ts.as_ref().context("missing ts")?;
                                    run_return_screen = Screen::ManualSatellite;
                                    pending_playout = Some(
                                        manual_sat.to_play_params(ts.display().to_string()),
                                    );
                                    status.clear();
                                }
                                Err(msg) => status = msg.into(),
                            }
                        }
                    }
                    Screen::ManualTerrestrial => {
                        if let Some(result) = manual_terr.on_key(key.code) {
                            match result {
                                Ok(()) => {
                                    let ts = selected_ts.as_ref().context("missing ts")?;
                                    run_return_screen = Screen::ManualTerrestrial;
                                    pending_playout = Some(
                                        manual_terr.to_play_params(ts.display().to_string()),
                                    );
                                    status.clear();
                                }
                                Err(msg) => status = msg.into(),
                            }
                        }
                    }
                    Screen::ManualCable => {
                        if let Some(result) = manual_cable.on_key(key.code) {
                            match result {
                                Ok(()) => {
                                    let ts = selected_ts.as_ref().context("missing ts")?;
                                    run_return_screen = Screen::ManualCable;
                                    pending_playout = Some(
                                        manual_cable.to_play_params(ts.display().to_string()),
                                    );
                                    status.clear();
                                }
                                Err(msg) => status = msg.into(),
                            }
                        }
                    }
                    Screen::Run => {}
                }
            }
        }
    };

    disable_raw_mode()?;
    term.backend_mut().execute(LeaveAlternateScreen)?;
    term.show_cursor()?;
    if screen == Screen::Run && !run_state.finished {
        let _ = playout_child.as_mut().map(Child::kill);
    }
    result
}

fn run_playout_tui_auto(
    paths: TsduckPaths,
    params: PlayParams,
    modulator: Option<&DvbDevice>,
) -> anyhow::Result<()> {
    let mut stdout = stdout();
    enable_raw_mode()?;
    stdout.execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut term = Terminal::new(backend)?;
    let mut run_state = PlayoutRunState {
        status: "Starting…".into(),
        summary: String::new(),
        command: String::new(),
        logs: Vec::new(),
        finished: false,
        success: false,
    };
    let mut playout_rx = None;
    let mut playout_child = None;
    let mut confirm = ConfirmPrompt::default();
    begin_playout(
        &paths,
        &params,
        modulator,
        &mut run_state,
        &mut playout_rx,
        &mut playout_child,
    )?;

    let result = 'main: loop {
        if let Some(rx) = playout_rx.as_ref() {
            while let Ok(PlayoutMsg::Log(line)) = rx.try_recv() {
                run_state.push_log(line);
            }
        }
        poll_playout_child(&mut playout_child, &mut run_state);
        term.draw(|f| {
            draw_run(f, modulator, &run_state);
            confirm.draw(f);
        })?;
        if run_state.finished {
            break 'main if run_state.success {
                Ok(())
            } else {
                anyhow::bail!("playout failed")
            };
        }
        if event::poll(Duration::from_millis(150))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                if confirm.active {
                    if let Some(yes) = confirm.on_key(key.code) {
                        if yes && playout_active(&playout_child, &run_state) {
                            stop_playout(&mut playout_child, &mut run_state);
                            break 'main Ok(());
                        }
                    }
                    continue;
                }
                if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
                    if playout_active(&playout_child, &run_state) {
                        confirm.ask("Playout durdurulsun mu?");
                    } else {
                        break 'main Ok(());
                    }
                }
            }
        }
    };

    disable_raw_mode()?;
    term.backend_mut().execute(LeaveAlternateScreen)?;
    term.show_cursor()?;
    result
}

fn begin_playout(
    paths: &TsduckPaths,
    params: &PlayParams,
    modulator: Option<&DvbDevice>,
    run_state: &mut PlayoutRunState,
    playout_rx: &mut Option<Receiver<PlayoutMsg>>,
    playout_child: &mut Option<Child>,
) -> anyhow::Result<()> {
    if modulator.is_none() {
        anyhow::bail!("no modulator found — connect DTU-315");
    }
    let mhz = params.frequency_hz / 1_000_000;
    let freq = match params.delivery {
        PlayoutDelivery::Satellite => format!("IF {mhz} MHz"),
        PlayoutDelivery::Terrestrial | PlayoutDelivery::Cable => format!("RF {mhz} MHz"),
    };
    let mut tail = params.modulation.clone();
    if let Some(sr) = params.symbol_rate {
        tail.push_str(&format!(" · {} kSym/s", sr / 1_000));
    }
    if let Some(bw) = params.bandwidth_hz {
        tail.push_str(&format!(" · BW {} MHz", bw / 1_000_000));
    }
    if let Some(cr) = &params.code_rate {
        tail.push_str(&format!(" · FEC {cr}"));
    }
    if let Some(j) = &params.j83 {
        tail.push_str(&format!(" · J.83 {j}"));
    }
    run_state.summary = format!(
        "{} → {} {}{}",
        crate::tui::widgets::truncate_end(&params.input, 72),
        freq,
        tail,
        if params.infinite { " · loop" } else { "" }
    );
    if let Some(m) = modulator {
        run_state.push_log(format!("Modulator: {} @ {}", m.model, m.address));
    }
    let args = params.to_tsp_args();
    run_state.command = tsduck::display_command("tsp", &args);
    run_state.push_log(format!("$ {}", run_state.command));
    run_state.status = "Running tsp…".into();
    run_state.finished = false;
    run_state.success = false;

    let (tx, rx) = mpsc::channel();
    *playout_rx = Some(rx);

    let paths = paths.clone();
    let args = args.clone();
    let mut child = tsduck::tool_command(&paths, "tsp")
        .arg("--verbose")
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to launch tsp")?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let tx_out = tx.clone();
    if let Some(out) = stdout {
        thread::spawn(move || pump_lines(out, tx_out));
    }
    if let Some(err) = stderr {
        thread::spawn(move || pump_lines(err, tx));
    }
    *playout_child = Some(child);
    Ok(())
}

fn poll_playout_child(child: &mut Option<Child>, run_state: &mut PlayoutRunState) {
    let Some(c) = child.as_mut() else {
        return;
    };
    match c.try_wait() {
        Ok(Some(status)) => {
            run_state.finished = true;
            run_state.success = status.success();
            run_state.status = if status.success() {
                "Playout finished".into()
            } else {
                "Playout failed".into()
            };
            run_state.push_log(format!("tsp exited: {status}"));
            *child = None;
        }
        Ok(None) => {}
        Err(e) => {
            run_state.finished = true;
            run_state.success = false;
            run_state.status = "Playout failed".into();
            run_state.push_log(format!("wait error: {e}"));
            *child = None;
        }
    }
}

fn pump_lines<R: std::io::Read + Send + 'static>(reader: R, tx: mpsc::Sender<PlayoutMsg>) {
    for line in BufReader::new(reader).lines().map_while(Result::ok) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if tx.send(PlayoutMsg::Log(trimmed.to_string())).is_err() {
            break;
        }
    }
}

fn stop_playout(child: &mut Option<Child>, run_state: &mut PlayoutRunState) {
    if let Some(c) = child.as_mut() {
        let _ = c.kill();
        let _ = c.wait();
    }
    *child = None;
    run_state.finished = true;
    run_state.success = true;
    run_state.status = "Stopped".into();
    run_state.push_log("Stopped by user (q)".to_string());
}

fn draw_run(frame: &mut Frame, modulator: Option<&DvbDevice>, st: &PlayoutRunState) {
    let chunks = Layout::vertical([
        Constraint::Length(5),
        Constraint::Min(6),
        Constraint::Length(KEYS_FOOTER_HEIGHT),
    ])
    .split(frame.area());
    let mod_line = modulator
        .map(|m| format!("{} @ {}", m.model, m.address))
        .unwrap_or_else(|| "NOT FOUND".into());
    let status_body = format!(
        "{}\n{}\n{}",
        mod_line, st.summary, st.status
    );
    paragraph(frame, chunks[0], "Playout", &status_body);
    log_panel(frame, chunks[1], "tsp log", &st.logs);
    let keys = if st.finished {
        "Esc back · q quit"
    } else {
        "Esc/q stop · Y/N confirm"
    };
    paragraph(frame, chunks[2], "Keys", keys);
}

fn open_manual_forms_from_tune(
    tune: Option<&TuneRecord>,
    manual_sat: &mut ManualPlayoutSatelliteForm,
    manual_terr: &mut ManualPlayoutTerrestrialForm,
    manual_cable: &mut ManualPlayoutCableForm,
    delivery_sel: &mut usize,
    screen: &mut Screen,
) {
    if let Some(tune) = tune {
        let delivery = delivery_from_tune(tune);
        *delivery_sel = match delivery {
            DeliveryChoice::Satellite => 0,
            DeliveryChoice::Terrestrial => 1,
            DeliveryChoice::Cable => 2,
        };
        match delivery {
            DeliveryChoice::Satellite => {
                *manual_sat = ManualPlayoutSatelliteForm::from_tune(tune);
                *screen = Screen::ManualSatellite;
            }
            DeliveryChoice::Terrestrial => {
                *manual_terr = ManualPlayoutTerrestrialForm::from_tune(tune);
                *screen = Screen::ManualTerrestrial;
            }
            DeliveryChoice::Cable => {
                *manual_cable = ManualPlayoutCableForm::from_tune(tune);
                *screen = Screen::ManualCable;
            }
        }
    } else {
        *screen = Screen::ManualDelivery;
    }
}

fn draw_manual_delivery(
    frame: &mut Frame,
    selected: usize,
    ts: Option<&PathBuf>,
    modulator: Option<&DvbDevice>,
    status: &str,
) {
    let rows = [
        DeliveryChoice::Satellite.label(),
        DeliveryChoice::Terrestrial.label(),
        DeliveryChoice::Cable.label(),
    ];
    let area = frame.area();
    let footer_h = KEYS_FOOTER_HEIGHT + 2;
    let list_h = area.height.saturating_sub(footer_h);
    list_menu(
        frame,
        Rect::new(area.x, area.y, area.width, list_h),
        "Playout — delivery system",
        &rows.iter().map(|s| (*s).into()).collect::<Vec<_>>(),
        selected,
    );
    let file_line = ts
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let mod_line = modulator
        .map(|m| format!("{} @ {}", m.model, m.address))
        .unwrap_or_else(|| "NOT FOUND".into());
    let body = if status.is_empty() {
        format!("File: {file_line}\nModulator: {mod_line}\n\n↑↓ · Enter open form · Esc back")
    } else {
        format!("{status}\n\nFile: {file_line}\nModulator: {mod_line}\n\n↑↓ · Enter · Esc back")
    };
    paragraph(
        frame,
        Rect::new(area.x, area.y + list_h, area.width, footer_h),
        "Keys",
        &body,
    );
}

fn prepare_after_file_pick(
    ts: &PathBuf,
    screen: &mut Screen,
    services: &mut Option<CaptureServicesRecord>,
    status: &mut String,
) -> anyhow::Result<()> {
    let sidecar = services_json_path_for_ts(ts);
    if sidecar.is_file() {
        *services = Some(CaptureServicesRecord::read_file(&sidecar)?);
        *screen = Screen::FromServices;
        status.clear();
    } else {
        *services = None;
        *screen = Screen::ManualDelivery;
        *status = "no sidecar — pick delivery, then edit all parameters on one form".into();
    }
    Ok(())
}

fn playout_active(child: &Option<Child>, run_state: &PlayoutRunState) -> bool {
    child.is_some() && !run_state.finished
}

fn run_no_modulator_screen() -> anyhow::Result<()> {
    let mut stdout = stdout();
    enable_raw_mode()?;
    stdout.execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut term = Terminal::new(backend)?;
    loop {
        term.draw(|f| {
            paragraph(
                f,
                f.area(),
                "Playout",
                "DTU-315 modulator not found.\n\nConnect the DTU-315 and run again.\n\nEsc · q quit",
            );
        })?;
        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press
                    && matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
                {
                    break;
                }
            }
        }
    }
    disable_raw_mode()?;
    term.backend_mut().execute(LeaveAlternateScreen)?;
    term.show_cursor()?;
    lastlog::event("tui.playout", "end");
    Ok(())
}

fn load_playout_from_ts(ts: &PathBuf, infinite: bool) -> anyhow::Result<Option<PlayParams>> {
    let sidecar = services_json_path_for_ts(ts);
    if !sidecar.is_file() {
        return Ok(None);
    }
    let record = CaptureServicesRecord::read_file(&sidecar)?;
    Ok(Some(play_params_from_tune(
        ts.to_string_lossy().into_owned(),
        &record.tune,
        infinite,
    )?))
}

fn draw_from_services(
    frame: &mut Frame,
    modulator: Option<&DvbDevice>,
    ts: Option<&PathBuf>,
    services: Option<&CaptureServicesRecord>,
    status: &str,
) {
    let mod_line = modulator
        .map(|m| format!("{} @ {}", m.model, m.address))
        .unwrap_or_else(|| "NOT FOUND".into());
    let body = match (ts, services) {
        (Some(ts), Some(rec)) => {
            let t = &rec.tune;
            let sys = t.delivery_system.to_ascii_uppercase();
            let tune_line = if sys.starts_with("DVB-T") {
                format!(
                    "RF: {} MHz {} {} FEC {}\nPlayout: {} MHz DVB-T/T2 · Services: {}",
                    t.frequency_mhz,
                    t.delivery_system,
                    t.modulation,
                    t.fec,
                    t.frequency_mhz,
                    rec.services.len(),
                )
            } else if sys.starts_with("DVB-C") {
                format!(
                    "RF: {} MHz {} SR {} kS/s {} FEC {}\nPlayout: {} MHz {} · Services: {}",
                    t.frequency_mhz,
                    t.delivery_system,
                    t.symbol_rate_ksps,
                    t.modulation,
                    t.fec,
                    t.frequency_mhz,
                    t.modulation,
                    rec.services.len(),
                )
            } else {
                let if_mhz = crate::pipeline::l_band_if_mhz_universal(t.frequency_mhz);
                format!(
                    "Downlink: {} MHz {} · SR {} · {} {} FEC {}\n\
                     Playout IF: {} MHz · Services: {}",
                    t.frequency_mhz,
                    t.polarity,
                    t.symbol_rate_ksps,
                    t.delivery_system,
                    t.modulation,
                    t.fec,
                    if_mhz,
                    rec.services.len(),
                )
            };
            format!(
                "File: {}\nSidecar: {}\n{}\n{}",
                ts.display(),
                services_json_path_for_ts(ts).display(),
                tune_line,
                status
            )
        }
        _ => format!("Missing file or sidecar.\n{status}"),
    };
    let chunks = Layout::vertical([
        Constraint::Min(6),
        Constraint::Length(crate::tui::widgets::INFO_FOOTER_HEIGHT),
        Constraint::Length(crate::tui::widgets::KEYS_FOOTER_HEIGHT),
    ])
    .split(frame.area());
    paragraph(
        frame,
        chunks[0],
        "Playout — from capture metadata",
        &body,
    );
    paragraph(frame, chunks[1], "Modulator", &mod_line);
    paragraph(
        frame,
        chunks[2],
        "Keys",
        "Enter start · m manual · Esc back · q quit",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze::TuneRecord;
    use crate::pipeline::{dektec_modulation_from_tune, l_band_if_mhz_universal, PlayoutDelivery};
    use crate::tui::manual_playout_cable::ManualPlayoutCableForm;
    use ratatui::backend::TestBackend;

    #[test]
    fn cable_and_terrestrial_dektec_tokens() {
        let cable = TuneRecord {
            frequency_mhz: 722,
            polarity: String::new(),
            symbol_rate_ksps: 6876,
            delivery_system: "DVB-C".into(),
            modulation: "256-QAM".into(),
            fec: "auto".into(),
        };
        assert_eq!(
            dektec_modulation_from_tune(&cable).as_deref(),
            Some("256-QAM")
        );
        let terr = TuneRecord {
            frequency_mhz: 618,
            polarity: String::new(),
            symbol_rate_ksps: 0,
            delivery_system: "DVB-T2".into(),
            modulation: "64-QAM".into(),
            fec: "2/3".into(),
        };
        assert_eq!(
            dektec_modulation_from_tune(&terr).as_deref(),
            Some("DVB-T2")
        );
    }

    #[test]
    fn manual_cable_form_defaults() {
        let form = ManualPlayoutCableForm::new_default();
        let params = form.to_play_params("/tmp/x.ts".into());
        assert_eq!(params.frequency_hz, 722_000_000);
        assert_eq!(params.delivery, PlayoutDelivery::Cable);
        assert_eq!(params.j83.as_deref(), Some("A"));
        let args = params.to_tsp_args().join(" ");
        assert!(args.contains("-m 256-QAM"));
        assert!(args.contains("--symbol-rate 6876000"));
        assert!(args.contains("--j83 A"));
        assert!(!args.contains("-r "));
    }

    #[test]
    fn tune_to_dektec_token() {
        let tune = TuneRecord {
            frequency_mhz: 11054,
            polarity: "vertical".into(),
            symbol_rate_ksps: 30000,
            delivery_system: "DVB-S2".into(),
            modulation: "8-PSK".into(),
            fec: "3/4".into(),
        };
        assert_eq!(
            dektec_modulation_from_tune(&tune).as_deref(),
            Some("DVB-S2-8PSK")
        );
        assert_eq!(l_band_if_mhz_universal(11054), 1304);
    }

    #[test]
    fn manual_delivery_renders() {
        let backend = TestBackend::new(80, 20);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| {
            draw_manual_delivery(f, 2, None, None, "");
        })
        .unwrap();
        assert!(format!("{:?}", term.backend().buffer()).contains("Cable"));
    }
}

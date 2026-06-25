//! Interactive playout wizard for the DTU-315 modulator.

use crate::analyze::{services_json_path_for_ts, CaptureServicesRecord};
use crate::config::{AppConfig, TsduckPaths};
use crate::devices::{DeviceKind, DvbDevice};
use crate::pipeline::{play_params_from_tune, PlayParams};
use crate::tui::confirm::ConfirmPrompt;
use crate::tui::file_picker::FilePicker;
use crate::tui::widgets::{list_menu, log_panel, paragraph, picker_layout, KEYS_FOOTER_HEIGHT};
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum DeliveryChoice {
    Satellite,
    Terrestrial,
    Cable,
}

impl DeliveryChoice {
    fn label(self) -> &'static str {
        match self {
            Self::Satellite => "Satellite (DVB-S / DVB-S2)",
            Self::Terrestrial => "Terrestrial (DVB-T / T2) — coming soon",
            Self::Cable => "Cable (DVB-C) — coming soon",
        }
    }

    fn supported(self) -> bool {
        matches!(self, Self::Satellite)
    }

    fn from_index(i: usize) -> Self {
        match i {
            0 => Self::Satellite,
            1 => Self::Terrestrial,
            _ => Self::Cable,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ManualStep {
    Delivery,
    Frequency,
    Modulation,
    SymbolRate,
    Fec,
    Loop,
    Review,
}

struct ManualForm {
    delivery_sel: usize,
    if_frequency_mhz: u32,
    modulation: String,
    modulation_sel: usize,
    symbol_rate_ksps: u32,
    code_rate: String,
    fec_sel: usize,
    infinite: bool,
    step: ManualStep,
}

impl ManualForm {
    fn new() -> Self {
        Self {
            delivery_sel: 0,
            if_frequency_mhz: 1200,
            modulation: MODULATIONS[0].into(),
            modulation_sel: 0,
            symbol_rate_ksps: SYMBOL_RATES[0],
            code_rate: FECS[0].into(),
            fec_sel: 0,
            infinite: true,
            step: ManualStep::Delivery,
        }
    }

    fn to_play_params(&self, input: String) -> PlayParams {
        PlayParams {
            input,
            infinite: self.infinite,
            frequency_hz: self.if_frequency_mhz as u64 * 1_000_000,
            modulation: self.modulation.clone(),
            symbol_rate: self.symbol_rate_ksps as u64 * 1_000,
            code_rate: self.code_rate.clone(),
            level_dbm_tenths: None,
            instant_detach: !self.infinite,
        }
    }

    fn step_title(&self) -> &'static str {
        match self.step {
            ManualStep::Delivery => "Playout — delivery system",
            ManualStep::Frequency => "Playout — IF frequency (MHz)",
            ManualStep::Modulation => "Playout — modulation",
            ManualStep::SymbolRate => "Playout — symbol rate (kSym/s)",
            ManualStep::Fec => "Playout — FEC",
            ManualStep::Loop => "Playout — loop file",
            ManualStep::Review => "Playout — confirm & start",
        }
    }

    fn rows(&self) -> Vec<String> {
        match self.step {
            ManualStep::Delivery => vec![
                DeliveryChoice::Satellite.label().into(),
                DeliveryChoice::Terrestrial.label().into(),
                DeliveryChoice::Cable.label().into(),
            ],
            ManualStep::Frequency => vec![format!("IF frequency: {} MHz", self.if_frequency_mhz)],
            ManualStep::Modulation => MODULATIONS.iter().map(|m| (*m).into()).collect(),
            ManualStep::SymbolRate => SYMBOL_RATES
                .iter()
                .map(|sr| format!("{sr} kSym/s"))
                .collect(),
            ManualStep::Fec => FECS.iter().map(|f| (*f).into()).collect(),
            ManualStep::Loop => vec![format!(
                "Loop forever: {}",
                if self.infinite { "yes" } else { "no" }
            )],
            ManualStep::Review => vec![
                format!("Delivery: {}", DeliveryChoice::from_index(self.delivery_sel).label()),
                format!("IF: {} MHz", self.if_frequency_mhz),
                format!("Modulation: {}", self.modulation),
                format!("Symbol rate: {} kSym/s", self.symbol_rate_ksps),
                format!("FEC: {}", self.code_rate),
                format!("Loop: {}", if self.infinite { "yes" } else { "no" }),
                "▶ Start playout".into(),
            ],
        }
    }

    fn selection_index(&self) -> usize {
        match self.step {
            ManualStep::Delivery => self.delivery_sel,
            ManualStep::Modulation => self.modulation_sel,
            ManualStep::SymbolRate => SYMBOL_RATES
                .iter()
                .position(|&s| s == self.symbol_rate_ksps)
                .unwrap_or(0),
            ManualStep::Fec => self.fec_sel,
            ManualStep::Review => 6,
            _ => 0,
        }
    }

    fn apply_list_nav(&mut self, key: KeyCode) -> bool {
        let rows = self.rows();
        let mut sel = self.selection_index();
        let max = rows.len().saturating_sub(1);
        match key {
            KeyCode::Up if sel > 0 => sel -= 1,
            KeyCode::Down if sel < max => sel += 1,
            _ => return false,
        }
        match self.step {
            ManualStep::Delivery => self.delivery_sel = sel,
            ManualStep::Modulation => {
                self.modulation_sel = sel;
                self.modulation = MODULATIONS[sel].into();
            }
            ManualStep::SymbolRate => self.symbol_rate_ksps = SYMBOL_RATES[sel],
            ManualStep::Fec => {
                self.fec_sel = sel;
                self.code_rate = FECS[sel].into();
            }
            _ => return false,
        }
        true
    }

    fn apply_edit(&mut self, key: KeyCode) -> bool {
        match self.step {
            ManualStep::Frequency => match key {
                KeyCode::Left => self.if_frequency_mhz = self.if_frequency_mhz.saturating_sub(1),
                KeyCode::Right => self.if_frequency_mhz = (self.if_frequency_mhz + 1).min(2150),
                KeyCode::Char('-') | KeyCode::Char('_') => {
                    self.if_frequency_mhz = self.if_frequency_mhz.saturating_sub(10)
                }
                KeyCode::Char('+') | KeyCode::Char('=') => {
                    self.if_frequency_mhz = (self.if_frequency_mhz + 10).min(2150)
                }
                _ => return false,
            },
            ManualStep::Loop => match key {
                KeyCode::Enter | KeyCode::Char(' ') => self.infinite = !self.infinite,
                _ => return false,
            },
            _ => return false,
        }
        true
    }

    fn advance(&mut self) -> Result<(), &'static str> {
        if self.step == ManualStep::Delivery {
            let choice = DeliveryChoice::from_index(self.delivery_sel);
            if !choice.supported() {
                return Err("only DVB-S/S2 playout is implemented");
            }
        }
        self.step = match self.step {
            ManualStep::Delivery => ManualStep::Frequency,
            ManualStep::Frequency => ManualStep::Modulation,
            ManualStep::Modulation => ManualStep::SymbolRate,
            ManualStep::SymbolRate => ManualStep::Fec,
            ManualStep::Fec => ManualStep::Loop,
            ManualStep::Loop => ManualStep::Review,
            ManualStep::Review => ManualStep::Review,
        };
        Ok(())
    }

    fn retreat(&mut self) {
        self.step = match self.step {
            ManualStep::Delivery => ManualStep::Delivery,
            ManualStep::Frequency => ManualStep::Delivery,
            ManualStep::Modulation => ManualStep::Frequency,
            ManualStep::SymbolRate => ManualStep::Modulation,
            ManualStep::Fec => ManualStep::SymbolRate,
            ManualStep::Loop => ManualStep::Fec,
            ManualStep::Review => ManualStep::Loop,
        };
    }
}

const MODULATIONS: &[&str] = &["DVB-S2-8PSK", "DVB-S2-QPSK", "DVB-S-QPSK"];
const SYMBOL_RATES: &[u32] = &[30000, 27500, 22000, 20000];
const FECS: &[&str] = &["3/4", "5/6", "2/3"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    FilePick,
    FromServices,
    Manual,
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
        let params = load_playout_from_ts(&path, true)?
            .unwrap_or_else(|| ManualForm::new().to_play_params(ts));
        return run_playout_tui_auto(paths, params, modulator.as_ref());
    }

    let mut screen = Screen::FilePick;
    let mut picker = FilePicker::open(Some(AppConfig::load().capture_dir));
    let mut selected_ts: Option<PathBuf> = input.map(PathBuf::from);
    let mut services: Option<CaptureServicesRecord> = None;
    let mut manual = ManualForm::new();
    let mut list_sel = 0usize;
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
        prepare_after_file_pick(
            ts,
            &mut screen,
            &mut services,
            &mut manual,
            &mut manual_return,
            &mut status,
        )?;
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
                Screen::Manual => {
                    draw_manual(
                        f,
                        modulator.as_ref(),
                        selected_ts.as_ref(),
                        &manual,
                        list_sel,
                        &status,
                    )
                }
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
                        Screen::Manual if manual.step == ManualStep::Delivery => {
                            screen = manual_return;
                            status.clear();
                        }
                        Screen::Manual => {
                            manual.retreat();
                            list_sel = 0;
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
                                &mut manual,
                                &mut manual_return,
                                &mut status,
                            )?;
                            list_sel = 0;
                        }
                    }
                    Screen::FromServices => match key.code {
                        KeyCode::Enter => {
                            let ts = selected_ts.as_ref().context("missing ts")?;
                            run_return_screen = Screen::FromServices;
                            pending_playout = load_playout_from_ts(ts, true)?;
                        }
                        KeyCode::Char('m') => {
                            manual_return = Screen::FromServices;
                            manual = ManualForm::new();
                            screen = Screen::Manual;
                            status = "manual parameters".into();
                            list_sel = 0;
                        }
                        _ => {}
                    },
                    Screen::Manual => {
                        list_sel = manual.selection_index();
                        if manual.apply_list_nav(key.code) || manual.apply_edit(key.code) {
                            continue;
                        }
                        match key.code {
                            KeyCode::Left => {
                                manual.retreat();
                                list_sel = 0;
                            }
                            KeyCode::Enter => {
                                if manual.step == ManualStep::Review && manual.selection_index() == 6
                                {
                                    let ts = selected_ts.as_ref().context("missing ts")?;
                                    run_return_screen = Screen::Manual;
                                    pending_playout =
                                        Some(manual.to_play_params(ts.display().to_string()));
                                } else if let Err(msg) = manual.advance() {
                                    status = msg.into();
                                } else {
                                    list_sel = 0;
                                    status.clear();
                                }
                            }
                            _ => {}
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
    let if_mhz = params.frequency_hz / 1_000_000;
    run_state.summary = format!(
        "{} → {} MHz {} · {} kSym/s FEC {}{}",
        crate::tui::widgets::truncate_end(&params.input, 72),
        if_mhz,
        params.modulation,
        params.symbol_rate / 1_000,
        params.code_rate,
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

fn prepare_after_file_pick(
    ts: &PathBuf,
    screen: &mut Screen,
    services: &mut Option<CaptureServicesRecord>,
    manual: &mut ManualForm,
    manual_return: &mut Screen,
    status: &mut String,
) -> anyhow::Result<()> {
    let sidecar = services_json_path_for_ts(ts);
    if sidecar.is_file() {
        *services = Some(CaptureServicesRecord::read_file(&sidecar)?);
        *screen = Screen::FromServices;
        *manual_return = Screen::FilePick;
        status.clear();
    } else {
        *services = None;
        *manual = ManualForm::new();
        *screen = Screen::Manual;
        *manual_return = Screen::FilePick;
        *status = "no sidecar *_services.json — enter parameters manually".into();
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
            let if_mhz = crate::pipeline::l_band_if_mhz_universal(t.frequency_mhz);
            format!(
                "File: {}\nSidecar: {}\nDownlink: {} MHz {} · SR {} · {} {} FEC {}\n\
                 Playout IF: {} MHz · Services: {}\n{}",
                ts.display(),
                services_json_path_for_ts(ts).display(),
                t.frequency_mhz,
                t.polarity,
                t.symbol_rate_ksps,
                t.delivery_system,
                t.modulation,
                t.fec,
                if_mhz,
                rec.services.len(),
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

fn draw_manual(
    frame: &mut Frame,
    modulator: Option<&DvbDevice>,
    ts: Option<&PathBuf>,
    form: &ManualForm,
    selected: usize,
    status: &str,
) {
    let [list_area, info_area, keys_area] = picker_layout(frame.area());
    let rows = form.rows();
    let sel = selected.min(rows.len().saturating_sub(1));
    list_menu(frame, list_area, form.step_title(), &rows, sel);
    let info = match (ts, modulator) {
        (Some(p), Some(m)) => format!("{} · {} @ {}", p.display(), m.model, m.address),
        (Some(p), None) => format!("{} · modulator NOT FOUND", p.display()),
        _ => String::new(),
    };
    let info_body = if status.is_empty() {
        info.clone()
    } else {
        status.to_string()
    };
    paragraph(frame, info_area, "File", &info_body);
    let keys_hint = if info.is_empty() {
        "↑↓ · Enter next/start · ← step back · Esc back · q quit".to_string()
    } else {
        format!(
            "↑↓ · Enter · ← back · q · {}",
            crate::tui::widgets::truncate_end(&info, keys_area.width.saturating_sub(4) as usize)
        )
    };
    paragraph(frame, keys_area, "Keys", &keys_hint);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze::TuneRecord;
    use crate::pipeline::{dektec_modulation_from_tune, l_band_if_mhz_universal};
    use ratatui::backend::TestBackend;

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
    fn manual_form_renders() {
        let backend = TestBackend::new(80, 20);
        let mut term = Terminal::new(backend).unwrap();
        let form = ManualForm::new();
        term.draw(|f| list_menu(f, f.area(), form.step_title(), &form.rows(), 0))
            .unwrap();
        assert!(format!("{:?}", term.backend().buffer()).contains("delivery"));
    }
}

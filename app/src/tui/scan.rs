//! Channel scan screens for the watch TUI: a one-page form and a live progress view.
//!
//! The scan itself is [`crate::channel_scan`] (same engine as `dvbhub scan` and
//! the web UI), run on a worker thread; events arrive over a channel.

use crate::channel_scan::{self, ScanEvent, ScanOptions, ScanTarget, TargetResult};
use crate::channels::SavedChannel;
use crate::config::TsduckPaths;
use crate::devices::{DeliveryCategory, Discovery};
use crate::satellites::SatelliteDb;
use crate::tui::widgets::{log_panel, paragraph, KEYS_FOOTER_HEIGHT};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::Gauge;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread::JoinHandle;

const CABLE_MODS: &[&str] = &["256-QAM", "64-QAM", "128-QAM"];

/// Form rows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Row {
    Delivery,
    Satellite,
    Frequencies,
    SymbolRate,
    Modulation,
    Radio,
    Start,
}

/// What the form wants the caller to do after a key.
pub enum FormAction {
    None,
    Back,
    Start,
}

/// One-page scan form.
pub struct ScanForm {
    deliveries: Vec<DeliveryCategory>,
    sel_delivery: usize,
    satellites: Vec<String>,
    sel_sat: usize,
    freqs: String,
    symbol_rate: String,
    sel_mod: usize,
    radio: bool,
    row: usize,
    pub message: String,
}

impl ScanForm {
    /// Builds the form for the deliveries the connected tuners support.
    pub fn new(discovery: &Discovery) -> Self {
        let deliveries: Vec<DeliveryCategory> = [
            DeliveryCategory::Satellite,
            DeliveryCategory::Terrestrial,
            DeliveryCategory::Cable,
        ]
        .into_iter()
        .filter(|c| discovery.tuner_for(*c, None).is_some())
        .collect();
        Self {
            deliveries,
            sel_delivery: 0,
            satellites: SatelliteDb::load().satellites.into_iter().map(|s| s.name).collect(),
            sel_sat: 0,
            freqs: String::new(),
            symbol_rate: "6875".into(),
            sel_mod: 0,
            radio: true,
            row: 0,
            message: String::new(),
        }
    }

    fn delivery(&self) -> Option<DeliveryCategory> {
        self.deliveries.get(self.sel_delivery).copied()
    }

    fn rows(&self) -> Vec<Row> {
        let mut rows = vec![Row::Delivery];
        match self.delivery() {
            Some(DeliveryCategory::Satellite) => rows.push(Row::Satellite),
            Some(DeliveryCategory::Terrestrial) => rows.push(Row::Frequencies),
            Some(DeliveryCategory::Cable) => {
                rows.extend([Row::Frequencies, Row::SymbolRate, Row::Modulation])
            }
            None => {}
        }
        rows.extend([Row::Radio, Row::Start]);
        rows
    }

    /// Handles a key press.
    pub fn on_key(&mut self, key: KeyEvent) -> FormAction {
        let rows = self.rows();
        self.row = self.row.min(rows.len() - 1);
        let row = rows[self.row];
        let step = |v: usize, len: usize, d: i32| -> usize {
            if len == 0 {
                0
            } else {
                ((v as i32 + d).rem_euclid(len as i32)) as usize
            }
        };
        match key.code {
            KeyCode::Esc => return FormAction::Back,
            KeyCode::Up => self.row = self.row.saturating_sub(1),
            KeyCode::Down | KeyCode::Tab => self.row = (self.row + 1).min(rows.len() - 1),
            KeyCode::Left | KeyCode::Right => {
                let d = if key.code == KeyCode::Left { -1 } else { 1 };
                match row {
                    Row::Delivery => {
                        self.sel_delivery = step(self.sel_delivery, self.deliveries.len(), d);
                        self.row = 0;
                    }
                    Row::Satellite => self.sel_sat = step(self.sel_sat, self.satellites.len(), d),
                    Row::Modulation => self.sel_mod = step(self.sel_mod, CABLE_MODS.len(), d),
                    Row::Radio => self.radio = !self.radio,
                    _ => {}
                }
            }
            KeyCode::Char(c) if matches!(row, Row::Frequencies) && (c.is_ascii_digit() || c == ' ' || c == ',') => {
                self.freqs.push(c)
            }
            KeyCode::Char(c) if matches!(row, Row::SymbolRate) && c.is_ascii_digit() => self.symbol_rate.push(c),
            KeyCode::Backspace => match row {
                Row::Frequencies => {
                    self.freqs.pop();
                }
                Row::SymbolRate => {
                    self.symbol_rate.pop();
                }
                _ => {}
            },
            KeyCode::Enter => {
                if row == Row::Start {
                    return FormAction::Start;
                }
                self.row = (self.row + 1).min(rows.len() - 1);
            }
            _ => {}
        }
        FormAction::None
    }

    /// Builds the scan targets and options (errors are shown on the form).
    pub fn targets(&self, discovery: &Discovery) -> anyhow::Result<(Vec<ScanTarget>, ScanOptions, String)> {
        let cat = self.delivery().ok_or_else(|| anyhow::anyhow!("no tuner connected"))?;
        let dev = discovery
            .tuner_for(cat, None)
            .ok_or_else(|| anyhow::anyhow!("no {} tuner", cat.as_str()))?;
        let device = dev.capture_address(cat);
        let model = Some(dev.label.as_str());
        let freqs: Vec<u32> = self
            .freqs
            .split([' ', ','])
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        let targets = match cat {
            DeliveryCategory::Satellite => {
                let name = self
                    .satellites
                    .get(self.sel_sat)
                    .ok_or_else(|| anyhow::anyhow!("no satellite in the catalogue"))?;
                let db = SatelliteDb::load();
                let sat = db
                    .satellites
                    .iter()
                    .find(|s| &s.name == name)
                    .ok_or_else(|| anyhow::anyhow!("satellite {name} not found"))?;
                channel_scan::satellite_targets(sat, &device, model, &[])
            }
            DeliveryCategory::Terrestrial => channel_scan::terrestrial_targets(&device, model, &freqs, 8),
            DeliveryCategory::Cable => {
                anyhow::ensure!(!freqs.is_empty(), "enter at least one cable frequency");
                let sr = self.symbol_rate.parse().unwrap_or(6875);
                channel_scan::cable_targets(&device, model, &freqs, sr, CABLE_MODS[self.sel_mod])
            }
        };
        anyhow::ensure!(!targets.is_empty(), "nothing to scan");
        let opts = ScanOptions {
            include_radio: self.radio,
            ..Default::default()
        };
        Ok((targets, opts, format!("{} @ {device}", dev.label)))
    }

    /// Draws the form.
    pub fn draw(&self, f: &mut Frame, area: Rect) {
        let chunks = Layout::vertical([Constraint::Min(6), Constraint::Length(KEYS_FOOTER_HEIGHT)]).split(area);
        let rows = self.rows();
        let lines: Vec<String> = rows
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let mark = if i == self.row { "▶" } else { " " };
                let value = match r {
                    Row::Delivery => format!(
                        "Delivery      ◀ {} ▶",
                        self.delivery().map(|d| d.as_str()).unwrap_or("no tuner")
                    ),
                    Row::Satellite => format!(
                        "Satellite     ◀ {} ▶",
                        self.satellites.get(self.sel_sat).map(String::as_str).unwrap_or("—")
                    ),
                    Row::Frequencies => format!(
                        "Frequencies   {}_   (MHz{})",
                        self.freqs,
                        if self.delivery() == Some(DeliveryCategory::Terrestrial) {
                            "; empty = UHF 21–69"
                        } else {
                            ""
                        }
                    ),
                    Row::SymbolRate => format!("Symbol rate   {}_ kS/s", self.symbol_rate),
                    Row::Modulation => format!("Modulation    ◀ {} ▶", CABLE_MODS[self.sel_mod]),
                    Row::Radio => format!("Radio         ◀ {} ▶", if self.radio { "include" } else { "skip" }),
                    Row::Start => "[ Start scan ]".to_string(),
                };
                format!("{mark} {value}")
            })
            .collect();
        let body = format!(
            "{}\n\nFree-to-air TV (and radio) services are added to the channel list.\n{}",
            lines.join("\n"),
            self.message
        );
        paragraph(f, chunks[0], "Scan for channels", &body);
        paragraph(
            f,
            chunks[1],
            "keys",
            "↑↓ row · ←→ change · type frequencies · Enter next / start · Esc back",
        );
    }
}

/// A running scan.
pub struct ScanRun {
    rx: Receiver<ScanEvent>,
    cancel: Arc<AtomicBool>,
    handle: Option<JoinHandle<Vec<TargetResult>>>,
    targets: Vec<ScanTarget>,
    opts: ScanOptions,
    label: String,
    lines: Vec<String>,
    index: usize,
    total: usize,
    locked: usize,
    found: usize,
    pub confirm_stop: bool,
}

impl ScanRun {
    /// Starts the worker thread.
    pub fn start(paths: TsduckPaths, targets: Vec<ScanTarget>, opts: ScanOptions, label: String) -> Self {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let total = targets.len();
        let worker = {
            let (targets, opts, cancel) = (targets.clone(), opts.clone(), Arc::clone(&cancel));
            std::thread::spawn(move || {
                channel_scan::scan(&paths, &targets, &opts, &cancel, |ev| {
                    let _ = tx.send(ev);
                })
            })
        };
        Self {
            rx,
            cancel,
            handle: Some(worker),
            targets,
            opts,
            label,
            lines: Vec::new(),
            index: 0,
            total,
            locked: 0,
            found: 0,
            confirm_stop: false,
        }
    }

    /// Drains progress events; returns the channels when the scan has ended.
    pub fn poll(&mut self) -> Option<(Vec<SavedChannel>, usize, usize)> {
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                ScanEvent::TargetStart { index, .. } => self.index = index + 1,
                ScanEvent::TargetDone { result, .. } => {
                    let kept = result.wanted_count(&self.opts);
                    if result.locked {
                        self.locked += 1;
                        self.found += kept;
                    }
                    self.lines.push(format!(
                        "{:>3}/{} {:<24} {}",
                        result.index + 1,
                        self.total,
                        result.label,
                        if result.locked {
                            format!(
                                "LOCK SNR {} — {} services, {kept} kept",
                                result.snr_db.map(|v| format!("{v:.1} dB")).unwrap_or_else(|| "n/a".into()),
                                result.services.len()
                            )
                        } else {
                            "no lock".into()
                        }
                    ));
                }
            }
        }
        if self.handle.as_ref().is_some_and(|h| h.is_finished()) {
            let results = self.handle.take()?.join().unwrap_or_default();
            let channels = channel_scan::channels_from_results(&results, &self.targets, &self.opts);
            return Some((channels, self.locked, results.len()));
        }
        None
    }

    /// Asks the worker to stop after the current frequency.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// Draws progress.
    pub fn draw(&self, f: &mut Frame, area: Rect) {
        let chunks = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(KEYS_FOOTER_HEIGHT),
        ])
        .split(area);
        let ratio = if self.total == 0 {
            0.0
        } else {
            (self.index.saturating_sub(1) as f64 / self.total as f64).clamp(0.0, 1.0)
        };
        f.render_widget(
            Gauge::default()
                .block(ratatui::widgets::Block::bordered().title(format!("Scanning — {}", self.label)))
                .ratio(ratio)
                .label(format!(
                    "{}/{} · {} locked · {} channel(s)",
                    self.index, self.total, self.locked, self.found
                )),
            chunks[0],
        );
        log_panel(f, chunks[1], "Results", &self.lines);
        paragraph(
            f,
            chunks[2],
            "keys",
            if self.confirm_stop {
                "Stop the scan? y = stop (keeps channels found so far) · n = continue"
            } else {
                "Esc stop scan"
            },
        );
    }
}

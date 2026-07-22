//! Single-page manual DVB-C playout form (DTU-315).

use crate::analyze::TuneRecord;
use crate::pipeline::{PlayParams, PlayoutDelivery};
use crate::tui::widgets::{list_menu, paragraph, KEYS_FOOTER_HEIGHT};
use crossterm::event::KeyCode;
use ratatui::prelude::*;
use std::path::PathBuf;

const MODULATIONS: &[&str] = &["256-QAM", "64-QAM"];
const J83_ANNEX: &[&str] = &["A", "B", "C"];

const ROW_FREQUENCY: usize = 0;
const ROW_MODULATION: usize = 1;
const ROW_SYMBOL_RATE: usize = 2;
const ROW_J83: usize = 3;
const ROW_LOOP: usize = 4;
const ROW_START: usize = 5;
const ROW_COUNT: usize = 6;

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditField {
    Frequency,
    SymbolRate,
}

pub struct ManualPlayoutCableForm {
    frequency_mhz: u32,
    symbol_rate_ksps: u32,
    modulation_sel: usize,
    j83_sel: usize,
    infinite: bool,
    pub sel: usize,
    editing: Option<EditField>,
    edit_buffer: String,
}

impl ManualPlayoutCableForm {
    pub fn new_default() -> Self {
        Self {
            frequency_mhz: 722,
            symbol_rate_ksps: 6876,
            modulation_sel: 0,
            j83_sel: 0,
            infinite: true,
            sel: 0,
            editing: None,
            edit_buffer: String::new(),
        }
    }

    pub fn from_tune(tune: &TuneRecord) -> Self {
        let mut form = Self::new_default();
        form.frequency_mhz = tune.frequency_mhz;
        if tune.symbol_rate_ksps > 0 {
            form.symbol_rate_ksps = tune.symbol_rate_ksps;
        }
        if let Some(i) = MODULATIONS
            .iter()
            .position(|m| m.eq_ignore_ascii_case(&tune.modulation))
        {
            form.modulation_sel = i;
        }
        form
    }

    pub fn editing_active(&self) -> bool {
        self.editing.is_some()
    }

    pub fn cancel_edit(&mut self) {
        self.editing = None;
        self.edit_buffer.clear();
    }

    pub fn on_key(&mut self, key: KeyCode) -> Option<Result<(), &'static str>> {
        if let Some(field) = self.editing {
            self.apply_edit_key(field, key);
            return None;
        }

        match key {
            KeyCode::Up if self.sel > 0 => self.sel -= 1,
            KeyCode::Down if self.sel + 1 < ROW_COUNT => self.sel += 1,
            KeyCode::Left if self.is_choice_row(self.sel) => self.adjust_choice(-1),
            KeyCode::Right if self.is_choice_row(self.sel) => self.adjust_choice(1),
            KeyCode::Char(' ') if self.sel == ROW_LOOP => self.infinite = !self.infinite,
            KeyCode::Enter => match self.sel {
                ROW_FREQUENCY => {
                    self.begin_edit(EditField::Frequency, self.frequency_mhz.to_string())
                }
                ROW_SYMBOL_RATE => self.begin_edit(
                    EditField::SymbolRate,
                    self.symbol_rate_ksps.to_string(),
                ),
                ROW_START => return Some(self.validate()),
                _ => {}
            },
            _ => {}
        }
        None
    }

    pub fn to_play_params(&self, input: String) -> PlayParams {
        PlayParams {
            input,
            infinite: self.infinite,
            delivery: PlayoutDelivery::Cable,
            frequency_hz: self.frequency_mhz as u64 * 1_000_000,
            modulation: MODULATIONS[self.modulation_sel].into(),
            symbol_rate: Some(self.symbol_rate_ksps as u64 * 1_000),
            code_rate: None,
            bandwidth_hz: None,
            guard_interval: None,
            plp_modulation: None,
            j83: Some(J83_ANNEX[self.j83_sel].into()),
            level_dbm_tenths: None,
            instant_detach: !self.infinite,
        }
    }

    fn validate(&self) -> Result<(), &'static str> {
        if self.frequency_mhz < 47 || self.frequency_mhz > 1000 {
            return Err("RF frequency must be 47–1000 MHz");
        }
        if self.symbol_rate_ksps < 1000 || self.symbol_rate_ksps > 10_000 {
            return Err("symbol rate must be 1000–10000 kS/s");
        }
        Ok(())
    }

    fn is_choice_row(&self, row: usize) -> bool {
        matches!(row, ROW_MODULATION | ROW_J83 | ROW_LOOP)
    }

    fn begin_edit(&mut self, field: EditField, buffer: String) {
        self.editing = Some(field);
        self.edit_buffer = buffer;
    }

    fn apply_edit_key(&mut self, field: EditField, key: KeyCode) {
        match key {
            KeyCode::Esc => self.cancel_edit(),
            KeyCode::Enter => self.commit_edit(field),
            KeyCode::Backspace => {
                self.edit_buffer.pop();
            }
            KeyCode::Char(c) if c.is_ascii_digit() && self.edit_buffer.len() < 5 => {
                self.edit_buffer.push(c);
            }
            _ => {}
        }
    }

    fn commit_edit(&mut self, field: EditField) {
        match field {
            EditField::Frequency => {
                if let Ok(v) = self.edit_buffer.parse::<u32>() {
                    self.frequency_mhz = v.clamp(47, 1000);
                }
            }
            EditField::SymbolRate => {
                if let Ok(v) = self.edit_buffer.parse::<u32>() {
                    self.symbol_rate_ksps = v.clamp(1000, 10_000);
                }
            }
        }
        self.cancel_edit();
    }

    fn adjust_choice(&mut self, delta: i32) {
        match self.sel {
            ROW_MODULATION => cycle_usize(&mut self.modulation_sel, MODULATIONS.len(), delta),
            ROW_J83 => cycle_usize(&mut self.j83_sel, J83_ANNEX.len(), delta),
            ROW_LOOP => self.infinite = !self.infinite,
            _ => {}
        }
    }

    fn rows(&self) -> Vec<String> {
        let freq = if self.editing == Some(EditField::Frequency) {
            format!("{}_", self.edit_buffer)
        } else {
            self.frequency_mhz.to_string()
        };
        let sr = if self.editing == Some(EditField::SymbolRate) {
            format!("{}_", self.edit_buffer)
        } else {
            self.symbol_rate_ksps.to_string()
        };
        vec![
            format!("RF frequency (MHz): {freq}"),
            format!("Modulation: {}", MODULATIONS[self.modulation_sel]),
            format!("Symbol rate (kS/s): {sr}"),
            format!("J.83 annex: {}", J83_ANNEX[self.j83_sel]),
            format!(
                "Loop forever: {}",
                if self.infinite { "yes" } else { "no" }
            ),
            "▶ Start playout".into(),
        ]
    }

    fn key_hint(&self) -> &'static str {
        match self.editing {
            Some(EditField::Frequency) => "Type MHz · Enter done · Esc cancel",
            Some(EditField::SymbolRate) => "Type kS/s · Enter done · Esc cancel",
            None if self.is_choice_row(self.sel) => "←→ change · Space toggles loop",
            None => "↑↓ select · Enter edit/start",
        }
    }
}

fn cycle_usize(value: &mut usize, len: usize, delta: i32) {
    if len == 0 {
        return;
    }
    *value = (*value as i32 + delta).rem_euclid(len as i32) as usize;
}

pub fn draw_manual_playout_cable(
    frame: &mut Frame,
    form: &ManualPlayoutCableForm,
    ts: Option<&PathBuf>,
    modulator: Option<&crate::devices::DvbDevice>,
    status: &str,
) {
    let area = frame.area();
    let footer_h = KEYS_FOOTER_HEIGHT + 3;
    let list_h = area.height.saturating_sub(footer_h);
    let list_area = Rect::new(area.x, area.y, area.width, list_h);
    let footer = Rect::new(area.x, area.y + list_h, area.width, footer_h);

    list_menu(
        frame,
        list_area,
        "Playout — DVB-C (manual)",
        &form.rows(),
        form.sel,
    );

    let file_line = ts
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let mod_line = modulator
        .map(|m| format!("{} @ {}", m.model, m.address))
        .unwrap_or_else(|| "modulator NOT FOUND".into());
    let mut footer_text = format!(
        "File: {file_line}\nModulator: {mod_line}\n{}\n\n↑↓ field · Enter edit/start · ←→ choice · Esc back · q quit",
        form.key_hint()
    );
    if !status.is_empty() {
        footer_text = format!("{status}\n{footer_text}");
    }
    paragraph(frame, footer, "Keys", &footer_text);
}

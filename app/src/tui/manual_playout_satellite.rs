//! Single-page manual DVB-S/S2 playout form (DTU-315 L-band IF).

use crate::analyze::TuneRecord;
use crate::pipeline::{l_band_if_mhz_universal, PlayParams, PlayoutDelivery};
use crate::tui::widgets::{list_menu, paragraph, KEYS_FOOTER_HEIGHT};
use crossterm::event::KeyCode;
use ratatui::prelude::*;
use std::path::PathBuf;

const MODULATIONS: &[&str] = &["DVB-S2-8PSK", "DVB-S2-QPSK", "DVB-S-QPSK"];
const SYMBOL_RATES: &[u32] = &[30000, 27500, 22000, 20000];
const FECS: &[&str] = &["3/4", "5/6", "2/3"];

const ROW_FREQUENCY: usize = 0;
const ROW_MODULATION: usize = 1;
const ROW_SYMBOL_RATE: usize = 2;
const ROW_FEC: usize = 3;
const ROW_LOOP: usize = 4;
const ROW_START: usize = 5;
const ROW_COUNT: usize = 6;

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditField {
    Frequency,
}

pub struct ManualPlayoutSatelliteForm {
    if_frequency_mhz: u32,
    modulation_sel: usize,
    symbol_rate_sel: usize,
    fec_sel: usize,
    infinite: bool,
    pub sel: usize,
    editing: Option<EditField>,
    edit_buffer: String,
}

impl ManualPlayoutSatelliteForm {
    pub fn new_default() -> Self {
        Self {
            if_frequency_mhz: 1200,
            modulation_sel: 0,
            symbol_rate_sel: 0,
            fec_sel: 0,
            infinite: true,
            sel: 0,
            editing: None,
            edit_buffer: String::new(),
        }
    }

    pub fn from_tune(tune: &TuneRecord) -> Self {
        let mut form = Self::new_default();
        form.if_frequency_mhz = l_band_if_mhz_universal(tune.frequency_mhz);
        if let Some(i) = MODULATIONS.iter().position(|m| {
            m.contains("8PSK") && tune.modulation.contains("8")
                || m.contains("QPSK") && tune.modulation.contains("QPSK")
        }) {
            form.modulation_sel = i;
        }
        if tune.symbol_rate_ksps > 0 {
            if let Some(i) = SYMBOL_RATES
                .iter()
                .position(|&s| s == tune.symbol_rate_ksps)
            {
                form.symbol_rate_sel = i;
            }
        }
        if let Some(i) = FECS.iter().position(|f| *f == tune.fec) {
            form.fec_sel = i;
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
                    self.begin_edit(EditField::Frequency, self.if_frequency_mhz.to_string())
                }
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
            delivery: PlayoutDelivery::Satellite,
            frequency_hz: self.if_frequency_mhz as u64 * 1_000_000,
            modulation: MODULATIONS[self.modulation_sel].into(),
            symbol_rate: Some(SYMBOL_RATES[self.symbol_rate_sel] as u64 * 1_000),
            code_rate: Some(FECS[self.fec_sel].into()),
            bandwidth_hz: None,
            guard_interval: None,
            plp_modulation: None,
            j83: None,
            level_dbm_tenths: None,
            instant_detach: !self.infinite,
        }
    }

    fn validate(&self) -> Result<(), &'static str> {
        if self.if_frequency_mhz < 950 || self.if_frequency_mhz > 2150 {
            return Err("IF frequency must be 950–2150 MHz");
        }
        Ok(())
    }

    fn is_choice_row(&self, row: usize) -> bool {
        matches!(row, ROW_MODULATION | ROW_SYMBOL_RATE | ROW_FEC | ROW_LOOP)
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
            KeyCode::Char(c) if c.is_ascii_digit() && self.edit_buffer.len() < 4 => {
                self.edit_buffer.push(c);
            }
            _ => {}
        }
    }

    fn commit_edit(&mut self, field: EditField) {
        if field == EditField::Frequency {
            if let Ok(v) = self.edit_buffer.parse::<u32>() {
                self.if_frequency_mhz = v.clamp(950, 2150);
            }
        }
        self.cancel_edit();
    }

    fn adjust_choice(&mut self, delta: i32) {
        match self.sel {
            ROW_MODULATION => cycle_usize(&mut self.modulation_sel, MODULATIONS.len(), delta),
            ROW_SYMBOL_RATE => cycle_usize(&mut self.symbol_rate_sel, SYMBOL_RATES.len(), delta),
            ROW_FEC => cycle_usize(&mut self.fec_sel, FECS.len(), delta),
            ROW_LOOP => self.infinite = !self.infinite,
            _ => {}
        }
    }

    fn rows(&self) -> Vec<String> {
        let freq = if self.editing == Some(EditField::Frequency) {
            format!("{}_", self.edit_buffer)
        } else {
            self.if_frequency_mhz.to_string()
        };
        vec![
            format!("IF frequency (MHz): {freq}"),
            format!("Modulation: {}", MODULATIONS[self.modulation_sel]),
            format!("Symbol rate: {} kSym/s", SYMBOL_RATES[self.symbol_rate_sel]),
            format!("FEC: {}", FECS[self.fec_sel]),
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

pub fn draw_manual_playout_satellite(
    frame: &mut Frame,
    form: &ManualPlayoutSatelliteForm,
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
        "Playout — DVB-S/S2 (manual)",
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

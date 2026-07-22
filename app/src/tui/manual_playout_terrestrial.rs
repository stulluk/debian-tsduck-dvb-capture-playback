//! Single-page manual DVB-T/T2 playout form (DTU-315).

use crate::analyze::TuneRecord;
use crate::pipeline::{PlayParams, PlayoutDelivery};
use crate::tui::widgets::{list_menu, paragraph, KEYS_FOOTER_HEIGHT};
use crossterm::event::KeyCode;
use ratatui::prelude::*;
use std::path::PathBuf;

const SYSTEMS: &[&str] = &["DVB-T2", "DVB-T"];
const BANDWIDTHS: &[u8] = &[8, 7, 6];
const GUARDS: &[&str] = &["19/256", "1/32", "1/16"];

const ROW_FREQUENCY: usize = 0;
const ROW_SYSTEM: usize = 1;
const ROW_BANDWIDTH: usize = 2;
const ROW_GUARD: usize = 3;
const ROW_LOOP: usize = 4;
const ROW_START: usize = 5;
const ROW_COUNT: usize = 6;

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditField {
    Frequency,
}

pub struct ManualPlayoutTerrestrialForm {
    frequency_mhz: u32,
    system_sel: usize,
    bandwidth_sel: usize,
    guard_sel: usize,
    infinite: bool,
    pub sel: usize,
    editing: Option<EditField>,
    edit_buffer: String,
}

impl ManualPlayoutTerrestrialForm {
    pub fn new_default() -> Self {
        Self {
            frequency_mhz: 618,
            system_sel: 0,
            bandwidth_sel: 0,
            guard_sel: 0,
            infinite: true,
            sel: 0,
            editing: None,
            edit_buffer: String::new(),
        }
    }

    pub fn from_tune(tune: &TuneRecord) -> Self {
        let mut form = Self::new_default();
        form.frequency_mhz = tune.frequency_mhz;
        if tune.delivery_system.eq_ignore_ascii_case("DVB-T") {
            form.system_sel = 1;
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
            delivery: PlayoutDelivery::Terrestrial,
            frequency_hz: self.frequency_mhz as u64 * 1_000_000,
            modulation: SYSTEMS[self.system_sel].into(),
            symbol_rate: None,
            code_rate: Some("2/3".into()),
            bandwidth_hz: Some(BANDWIDTHS[self.bandwidth_sel] as u64 * 1_000_000),
            guard_interval: Some(GUARDS[self.guard_sel].into()),
            plp_modulation: Some("64-QAM".into()),
            j83: None,
            level_dbm_tenths: None,
            instant_detach: !self.infinite,
        }
    }

    fn validate(&self) -> Result<(), &'static str> {
        if self.frequency_mhz < 174 || self.frequency_mhz > 862 {
            return Err("RF frequency must be 174–862 MHz");
        }
        Ok(())
    }

    fn is_choice_row(&self, row: usize) -> bool {
        matches!(row, ROW_SYSTEM | ROW_BANDWIDTH | ROW_GUARD | ROW_LOOP)
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
            KeyCode::Char(c) if c.is_ascii_digit() && self.edit_buffer.len() < 3 => {
                self.edit_buffer.push(c);
            }
            _ => {}
        }
    }

    fn commit_edit(&mut self, field: EditField) {
        if field == EditField::Frequency {
            if let Ok(v) = self.edit_buffer.parse::<u32>() {
                self.frequency_mhz = v.clamp(174, 862);
            }
        }
        self.cancel_edit();
    }

    fn adjust_choice(&mut self, delta: i32) {
        match self.sel {
            ROW_SYSTEM => cycle_usize(&mut self.system_sel, SYSTEMS.len(), delta),
            ROW_BANDWIDTH => cycle_usize(&mut self.bandwidth_sel, BANDWIDTHS.len(), delta),
            ROW_GUARD => cycle_usize(&mut self.guard_sel, GUARDS.len(), delta),
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
        vec![
            format!("RF frequency (MHz): {freq}"),
            format!("System: {}", SYSTEMS[self.system_sel]),
            format!("Bandwidth: {} MHz", BANDWIDTHS[self.bandwidth_sel]),
            format!("Guard interval: {}", GUARDS[self.guard_sel]),
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

pub fn draw_manual_playout_terrestrial(
    frame: &mut Frame,
    form: &ManualPlayoutTerrestrialForm,
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
        "Playout — DVB-T/T2 (manual)",
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

//! Single-page manual DVB-C capture form.

use crate::cable::CableMux;
use crate::tui::widgets::{list_menu, paragraph, KEYS_FOOTER_HEIGHT};
use crossterm::event::KeyCode;
use ratatui::prelude::*;

const DELIVERY: &[&str] = &["DVB-C", "DVB-C/A", "DVB-C/B", "DVB-C/C"];
const MODULATIONS: &[&str] = &["256-QAM", "64-QAM", "128-QAM", "32-QAM", "16-QAM"];
const FECS: &[&str] = &["auto", "2/3", "3/4", "5/6", "1/2"];

const ROW_LABEL: usize = 0;
const ROW_FREQUENCY: usize = 1;
const ROW_SYMBOL_RATE: usize = 2;
const ROW_DELIVERY: usize = 3;
const ROW_MODULATION: usize = 4;
const ROW_FEC: usize = 5;
const ROW_START: usize = 6;
const ROW_COUNT: usize = 7;

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditField {
    Label,
    Frequency,
    SymbolRate,
}

pub struct ManualCableForm {
    pub label: String,
    frequency_mhz: u32,
    symbol_rate_ksps: u32,
    delivery_sel: usize,
    modulation_sel: usize,
    fec_sel: usize,
    pub sel: usize,
    editing: Option<EditField>,
    edit_buffer: String,
}

impl ManualCableForm {
    pub fn new_default() -> Self {
        Self {
            label: String::new(),
            frequency_mhz: 722,
            symbol_rate_ksps: 6876,
            delivery_sel: 0,
            modulation_sel: 0,
            fec_sel: 0,
            sel: 0,
            editing: None,
            edit_buffer: String::new(),
        }
    }

    pub fn to_mux(&self) -> CableMux {
        CableMux {
            label: if self.label.trim().is_empty() {
                format!(
                    "{} MHz {} {} kS/s",
                    self.frequency_mhz, DELIVERY[self.delivery_sel], self.symbol_rate_ksps
                )
            } else {
                self.label.trim().to_string()
            },
            location: "manual".into(),
            frequency_mhz: self.frequency_mhz,
            symbol_rate_ksps: self.symbol_rate_ksps,
            delivery_system: DELIVERY[self.delivery_sel].into(),
            modulation: MODULATIONS[self.modulation_sel].into(),
            fec: Some(FECS[self.fec_sel].into()),
            source: "manual TUI entry".into(),
        }
    }

    pub fn editing_active(&self) -> bool {
        self.editing.is_some()
    }

    pub fn cancel_edit(&mut self) {
        self.editing = None;
        self.edit_buffer.clear();
    }

    fn is_choice_row(row: usize) -> bool {
        matches!(row, ROW_DELIVERY | ROW_MODULATION | ROW_FEC)
    }

    fn display_field(
        field: EditField,
        editing: Option<EditField>,
        buffer: &str,
        value: &str,
    ) -> String {
        if editing == Some(field) {
            format!("{buffer}_")
        } else {
            value.to_string()
        }
    }

    pub fn on_key(&mut self, key: KeyCode) -> Option<Result<(), &'static str>> {
        if let Some(field) = self.editing {
            let _ = self.apply_edit_key(field, key);
            return None;
        }

        match key {
            KeyCode::Up if self.sel > 0 => self.sel -= 1,
            KeyCode::Down if self.sel + 1 < ROW_COUNT => self.sel += 1,
            KeyCode::Left if Self::is_choice_row(self.sel) => self.adjust_choice(-1),
            KeyCode::Right if Self::is_choice_row(self.sel) => self.adjust_choice(1),
            KeyCode::Enter => match self.sel {
                ROW_LABEL => self.begin_edit(EditField::Label, self.label.clone()),
                ROW_FREQUENCY => {
                    self.begin_edit(EditField::Frequency, self.frequency_mhz.to_string())
                }
                ROW_SYMBOL_RATE => self.begin_edit(
                    EditField::SymbolRate,
                    self.symbol_rate_ksps.to_string(),
                ),
                ROW_START => return Some(self.validate_and_start()),
                _ => {}
            },
            _ => {}
        }
        None
    }

    fn validate_and_start(&self) -> Result<(), &'static str> {
        if self.frequency_mhz < 47 || self.frequency_mhz > 1000 {
            return Err("frequency must be 47–1000 MHz");
        }
        if self.symbol_rate_ksps < 1000 || self.symbol_rate_ksps > 10000 {
            return Err("symbol rate must be 1000–10000 kS/s");
        }
        Ok(())
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
            KeyCode::Char(c) => match field {
                EditField::Label if c.is_ascii_graphic() || c == ' ' => {
                    if self.edit_buffer.len() < 48 {
                        self.edit_buffer.push(c);
                    }
                }
                EditField::Frequency | EditField::SymbolRate if c.is_ascii_digit() => {
                    if self.edit_buffer.len() < 5 {
                        self.edit_buffer.push(c);
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn commit_edit(&mut self, field: EditField) {
        match field {
            EditField::Label => self.label = self.edit_buffer.trim().to_string(),
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
            ROW_DELIVERY => Self::cycle_usize(&mut self.delivery_sel, DELIVERY.len(), delta),
            ROW_MODULATION => Self::cycle_usize(&mut self.modulation_sel, MODULATIONS.len(), delta),
            ROW_FEC => Self::cycle_usize(&mut self.fec_sel, FECS.len(), delta),
            _ => {}
        }
    }

    fn cycle_usize(value: &mut usize, len: usize, delta: i32) {
        if len == 0 {
            return;
        }
        *value = (*value as i32 + delta).rem_euclid(len as i32) as usize;
    }

    fn rows(&self) -> Vec<String> {
        vec![
            format!(
                "Label: {}",
                Self::display_field(
                    EditField::Label,
                    self.editing,
                    &self.edit_buffer,
                    if self.label.is_empty() {
                        "(optional)"
                    } else {
                        &self.label
                    }
                )
            ),
            format!(
                "Frequency (MHz): {}",
                Self::display_field(
                    EditField::Frequency,
                    self.editing,
                    &self.edit_buffer,
                    &self.frequency_mhz.to_string()
                )
            ),
            format!(
                "Symbol rate (kS/s): {}",
                Self::display_field(
                    EditField::SymbolRate,
                    self.editing,
                    &self.edit_buffer,
                    &self.symbol_rate_ksps.to_string()
                )
            ),
            format!("Delivery: {}", DELIVERY[self.delivery_sel]),
            format!("Modulation: {}", MODULATIONS[self.modulation_sel]),
            format!("FEC: {}", FECS[self.fec_sel]),
            "▶ Start capture".into(),
        ]
    }

    fn key_hint(&self) -> &'static str {
        match self.editing {
            Some(EditField::Label) => "Type label · Enter done · Esc cancel",
            Some(EditField::Frequency) => "Type MHz · Enter done · Esc cancel",
            Some(EditField::SymbolRate) => "Type kS/s · Enter done · Esc cancel",
            None if Self::is_choice_row(self.sel) => "←→ change value",
            None => "↑↓ select · Enter edit/start · choice: ←→ · Esc back",
        }
    }
}

pub fn draw_manual_cable(frame: &mut Frame, form: &ManualCableForm, status: &str, device_hint: &str) {
    let area = frame.area();
    let footer_h = KEYS_FOOTER_HEIGHT + 2;
    let list_h = area.height.saturating_sub(footer_h);
    let list_area = Rect::new(area.x, area.y, area.width, list_h);
    let footer = Rect::new(area.x, area.y + list_h, area.width, footer_h);

    list_menu(
        frame,
        list_area,
        "Manual cable setup (DVB-C)",
        &form.rows(),
        form.sel,
    );

    let mut footer_text = format!(
        "Device: {device_hint}\n{}\n\n{}",
        form.key_hint(),
        "↑↓ field · Enter edit/start · choice: ←→ · Esc back · q quit"
    );
    if !status.is_empty() {
        footer_text = format!("{status}\n{footer_text}");
    }
    paragraph(frame, footer, "Keys", &footer_text);
}

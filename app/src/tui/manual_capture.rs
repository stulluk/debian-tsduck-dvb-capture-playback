//! Single-page manual capture form (custom satellite / transponder).

use crate::sat_capture::satellite_slug;
use crate::satellites::{Satellite, SatTransponder};
use crate::tui::lnb_presets::LNB_PRESETS;
use crate::tui::widgets::{list_menu, paragraph, KEYS_FOOTER_HEIGHT};
use crossterm::event::KeyCode;
use ratatui::prelude::*;

const POLARITIES: &[&str] = &["vertical", "horizontal", "left", "right"];
const MODULATIONS: &[&str] = &["QPSK", "8-PSK", "16-APSK"];
const FECS: &[&str] = &["3/4", "5/6", "2/3", "auto"];
const DELIVERY: &[&str] = &["DVB-S2", "DVB-S"];

const ROW_SAT_NAME: usize = 0;
const ROW_FREQUENCY: usize = 1;
const ROW_SYMBOL_RATE: usize = 2;
const ROW_POLARITY: usize = 3;
const ROW_MODULATION: usize = 4;
const ROW_FEC: usize = 5;
const ROW_DELIVERY: usize = 6;
const ROW_DISEQC_10: usize = 7;
const ROW_DISEQC_11: usize = 8;
const ROW_LNB: usize = 9;
const ROW_START: usize = 10;
const ROW_COUNT: usize = 11;

/// Text or numeric field currently being edited.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EditField {
    SatName,
    Frequency,
    SymbolRate,
}

/// User-entered capture parameters on one screen.
pub struct ManualCaptureForm {
    pub sat_name: String,
    frequency_mhz: u32,
    symbol_rate_ksps: u32,
    polarity_sel: usize,
    modulation_sel: usize,
    fec_sel: usize,
    delivery_sel: usize,
    /// DiSEqC 1.0 committed port (0–3).
    diseqc_committed: u8,
    /// DiSEqC 1.1 uncommitted port (0 = off, 1–16 on multiswitches).
    diseqc_uncommitted: u8,
    lnb_sel: usize,
    pub sel: usize,
    editing: Option<EditField>,
    edit_buffer: String,
}

impl ManualCaptureForm {
    /// Empty form with sensible defaults.
    pub fn new_default() -> Self {
        Self {
            sat_name: String::new(),
            frequency_mhz: 11054,
            symbol_rate_ksps: 30000,
            polarity_sel: 0,
            modulation_sel: 1,
            fec_sel: 0,
            delivery_sel: 0,
            diseqc_committed: 0,
            diseqc_uncommitted: 0,
            lnb_sel: 0, // Universal Ku (TSDuck "Extended", 10.7-12.75 GHz)
            sel: 0,
            editing: None,
            edit_buffer: String::new(),
        }
    }

    /// TSDuck `--satellite-number` (DiSEqC 1.0 low bits + 1.1 high nibble).
    pub fn satellite_number(&self) -> u32 {
        let committed = self.diseqc_committed.min(3) as u32;
        let uncommitted = self.diseqc_uncommitted.min(16) as u32;
        committed | (uncommitted.saturating_sub(1) << 2)
    }

    /// Builds catalogue objects from the form (no DB satellite required).
    pub fn to_satellite_and_tp(&self) -> (Satellite, SatTransponder) {
        let name = if self.sat_name.trim().is_empty() {
            "manual".into()
        } else {
            self.sat_name.trim().to_string()
        };
        let sat = Satellite {
            name,
            longitude: "manual".into(),
            lnb: self.lnb_tsp_value().into(),
            diseqc: self.satellite_number(),
            transponders: vec![],
        };
        let tp = SatTransponder {
            frequency_mhz: self.frequency_mhz,
            polarity: POLARITIES[self.polarity_sel].into(),
            symbol_rate_ksps: self.symbol_rate_ksps,
            modulation: MODULATIONS[self.modulation_sel].into(),
            fec: FECS[self.fec_sel].into(),
            delivery_system: DELIVERY[self.delivery_sel].into(),
            name: String::new(),
        };
        (sat, tp)
    }

    fn lnb_tsp_value(&self) -> &'static str {
        LNB_PRESETS
            .get(self.lnb_sel)
            .map(|p| p.tsp_value)
            .unwrap_or("Extended")
    }

    fn lnb_label(&self) -> &str {
        LNB_PRESETS
            .get(self.lnb_sel)
            .map(|p| p.label)
            .unwrap_or("Universal Ku (9750/10600, 10.7-12.75 GHz)")
    }

    fn is_choice_row(row: usize) -> bool {
        matches!(
            row,
            ROW_POLARITY
                | ROW_MODULATION
                | ROW_FEC
                | ROW_DELIVERY
                | ROW_DISEQC_10
                | ROW_DISEQC_11
                | ROW_LNB
        )
    }

    fn display_field(field: EditField, editing: Option<EditField>, buffer: &str, value: &str) -> String {
        if editing == Some(field) {
            buffer.to_string()
        } else {
            value.to_string()
        }
    }

    /// All editable rows for the list widget.
    pub fn rows(&self) -> Vec<String> {
        vec![
            format!(
                "sat_name: {}",
                Self::display_field(
                    EditField::SatName,
                    self.editing,
                    &self.edit_buffer,
                    &self.sat_name
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
                "Symbol rate (kSym/s): {}",
                Self::display_field(
                    EditField::SymbolRate,
                    self.editing,
                    &self.edit_buffer,
                    &self.symbol_rate_ksps.to_string()
                )
            ),
            format!("Polarity: {}", POLARITIES[self.polarity_sel]),
            format!("Modulation: {}", MODULATIONS[self.modulation_sel]),
            format!("FEC: {}", FECS[self.fec_sel]),
            format!("Delivery: {}", DELIVERY[self.delivery_sel]),
            format!("DiSEqC 1.0 port (0-3): {}", self.diseqc_committed),
            format!(
                "DiSEqC 1.1 port (0=off, 1-16): {}",
                self.diseqc_uncommitted
            ),
            format!("LNB: {}", self.lnb_label()),
            "▶ Start capture".into(),
        ]
    }

    pub fn editing_active(&self) -> bool {
        self.editing.is_some()
    }

    pub fn cancel_edit(&mut self) {
        self.editing = None;
        self.edit_buffer.clear();
    }

    /// Handles a key press. Returns `Some(Ok(()))` when capture should start.
    pub fn on_key(&mut self, key: KeyCode) -> Option<Result<(), &'static str>> {
        if let Some(field) = self.editing {
            return match self.apply_edit_key(field, key) {
                EditResult::Continue | EditResult::Committed => None,
            };
        }

        match key {
            KeyCode::Up if self.sel > 0 => self.sel -= 1,
            KeyCode::Down if self.sel + 1 < ROW_COUNT => self.sel += 1,
            KeyCode::Left if Self::is_choice_row(self.sel) => self.adjust_choice(-1),
            KeyCode::Right if Self::is_choice_row(self.sel) => self.adjust_choice(1),
            KeyCode::Enter => match self.sel {
                ROW_SAT_NAME => self.begin_edit(EditField::SatName, self.sat_name.clone()),
                ROW_FREQUENCY => {
                    self.begin_edit(EditField::Frequency, self.frequency_mhz.to_string())
                }
                ROW_SYMBOL_RATE => {
                    self.begin_edit(EditField::SymbolRate, self.symbol_rate_ksps.to_string())
                }
                ROW_START => return Some(self.validate_and_normalize()),
                _ => {}
            },
            _ => {}
        }
        None
    }

    fn begin_edit(&mut self, field: EditField, buffer: String) {
        self.editing = Some(field);
        self.edit_buffer = buffer;
    }

    fn apply_edit_key(&mut self, field: EditField, key: KeyCode) -> EditResult {
        match key {
            KeyCode::Esc => {
                self.cancel_edit();
                EditResult::Committed
            }
            KeyCode::Enter => self.commit_edit(field),
            KeyCode::Backspace => {
                self.edit_buffer.pop();
                EditResult::Continue
            }
            KeyCode::Char(c) => match field {
                EditField::SatName if c.is_ascii_alphanumeric() || c == '_' || c == '-' => {
                    self.edit_buffer.push(c);
                    EditResult::Continue
                }
                EditField::Frequency | EditField::SymbolRate if c.is_ascii_digit() => {
                    if self.edit_buffer.len() < 8 {
                        self.edit_buffer.push(c);
                    }
                    EditResult::Continue
                }
                _ => EditResult::Continue,
            },
            _ => EditResult::Continue,
        }
    }

    fn commit_edit(&mut self, field: EditField) -> EditResult {
        match field {
            EditField::SatName => {
                self.sat_name = self.edit_buffer.trim().to_string();
            }
            EditField::Frequency => {
                if let Ok(v) = self.edit_buffer.parse::<u32>() {
                    self.frequency_mhz = v.clamp(3000, 13000);
                }
            }
            EditField::SymbolRate => {
                if let Ok(v) = self.edit_buffer.parse::<u32>() {
                    self.symbol_rate_ksps = v.clamp(1000, 60000);
                }
            }
        }
        self.cancel_edit();
        EditResult::Committed
    }

    fn validate_and_normalize(&mut self) -> Result<(), &'static str> {
        if self.sat_name.trim().is_empty() {
            return Err("sat_name is required (used in .ts / _services.json names)");
        }
        self.sat_name = satellite_slug(&self.sat_name);
        if self.sat_name.is_empty() {
            return Err("sat_name must contain letters or digits");
        }
        if self.symbol_rate_ksps < 1000 {
            return Err("symbol rate must be at least 1000 kSym/s");
        }
        Ok(())
    }

    fn adjust_choice(&mut self, delta: i32) {
        match self.sel {
            ROW_POLARITY => Self::cycle_usize(&mut self.polarity_sel, POLARITIES.len(), delta),
            ROW_MODULATION => Self::cycle_usize(&mut self.modulation_sel, MODULATIONS.len(), delta),
            ROW_FEC => Self::cycle_usize(&mut self.fec_sel, FECS.len(), delta),
            ROW_DELIVERY => Self::cycle_usize(&mut self.delivery_sel, DELIVERY.len(), delta),
            ROW_DISEQC_10 => Self::cycle_u8(&mut self.diseqc_committed, 4, delta),
            ROW_DISEQC_11 => Self::cycle_u8(&mut self.diseqc_uncommitted, 17, delta),
            ROW_LNB => Self::cycle_usize(&mut self.lnb_sel, LNB_PRESETS.len(), delta),
            _ => {}
        }
    }

    fn cycle_u8(value: &mut u8, len: usize, delta: i32) {
        if len == 0 {
            return;
        }
        let next = (*value as i32 + delta).rem_euclid(len as i32) as u8;
        *value = next;
    }

    fn cycle_usize(value: &mut usize, len: usize, delta: i32) {
        if len == 0 {
            return;
        }
        let next = (*value as i32 + delta).rem_euclid(len as i32) as usize;
        *value = next;
    }
}

enum EditResult {
    Continue,
    Committed,
}

/// Draws the single-page manual capture form.
pub fn draw_manual_capture(frame: &mut Frame, form: &ManualCaptureForm, status: &str) {
    let area = frame.area();
    let body = if status.is_empty() {
        "DiSEqC 1.1 cascade: set 1.0 port (0-3) and 1.1 port (1-16, 0=off) — e.g. 1/8"
    } else {
        status
    };
    let header_h = 5u16.min(area.height);
    let header = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: header_h,
    };
    paragraph(
        frame,
        header,
        "Manual capture",
        &format!(
            "{body}\nFiles: capture_dir/<sat_name>/…/<sat_name>_<mhz>_<pol>_<sr>_<timestamp>.ts\n\n↑↓ select · Enter edit/start · choice: ←→"
        ),
    );
    let list_h = area
        .height
        .saturating_sub(header_h + KEYS_FOOTER_HEIGHT)
        .max(6);
    let list_area = Rect {
        x: area.x + 1,
        y: area.y + header_h,
        width: area.width.saturating_sub(2),
        height: list_h,
    };
    list_menu(
        frame,
        list_area,
        "Parameters",
        &form.rows(),
        form.sel,
    );
    let keys = if form.editing_active() {
        match form.editing {
            Some(EditField::SatName) => "Type sat_name · Enter done · Esc cancel · a-z 0-9 _ -",
            Some(EditField::Frequency) | Some(EditField::SymbolRate) => {
                "Type digits · Enter done · Esc cancel"
            }
            None => "",
        }
    } else {
        "↑↓ field · Enter edit/start · choice: ←→ · Esc back · q quit"
    };
    paragraph(
        frame,
        Rect {
            x: area.x,
            y: area.y + area.height.saturating_sub(KEYS_FOOTER_HEIGHT),
            width: area.width,
            height: KEYS_FOOTER_HEIGHT,
        },
        "Keys",
        keys,
    );
}

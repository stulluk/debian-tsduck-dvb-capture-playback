//! Single-page manual terrestrial capture form (DVB-T / DVB-T2).

use crate::terrestrial::TerrestrialMux;
use crate::tui::widgets::{list_menu, paragraph, KEYS_FOOTER_HEIGHT};
use crossterm::event::KeyCode;
use ratatui::prelude::*;

const DELIVERY: &[&str] = &["DVB-T2", "DVB-T"];
const BANDWIDTHS_MHZ: &[u8] = &[8, 7, 6];
const GUARD_INTERVALS: &[&str] = &["19/256", "19/128", "1/32", "1/16", "1/8", "1/4", "1/128"];
const MODULATIONS: &[&str] = &["64-QAM", "256-QAM", "16-QAM", "QPSK"];
const FECS: &[&str] = &["2/3", "3/4", "5/6", "1/2", "auto"];

const ROW_LABEL: usize = 0;
const ROW_FREQUENCY: usize = 1;
const ROW_DELIVERY: usize = 2;
const ROW_BANDWIDTH: usize = 3;
const ROW_GUARD: usize = 4;
const ROW_PLP: usize = 5;
const ROW_MODULATION: usize = 6;
const ROW_FEC: usize = 7;
const ROW_START: usize = 8;
const ROW_COUNT: usize = 9;

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditField {
    Label,
    Frequency,
}

/// Manual DVB-T/T2 tuning parameters.
pub struct ManualTerrestrialForm {
    pub label: String,
    frequency_mhz: u32,
    delivery_sel: usize,
    bandwidth_sel: usize,
    guard_sel: usize,
    plp: u8,
    modulation_sel: usize,
    fec_sel: usize,
    pub sel: usize,
    editing: Option<EditField>,
    edit_buffer: String,
}

impl ManualTerrestrialForm {
    /// Defaults aligned with Istanbul CH26 / RTÜK national plan.
    /// Pre-fills the form from a mux preset (test helper).
    #[cfg(test)]
    pub fn from_preset(preset: &TerrestrialMux) -> Self {
        let delivery_sel = if preset.delivery_system.eq_ignore_ascii_case("DVB-T") {
            1
        } else {
            0
        };
        let bandwidth_sel = BANDWIDTHS_MHZ
            .iter()
            .position(|b| *b == preset.bandwidth_mhz)
            .unwrap_or(0);
        let guard_sel = GUARD_INTERVALS
            .iter()
            .position(|g| *g == preset.guard_interval.as_str())
            .unwrap_or(0);
        let modulation_sel = preset
            .modulation
            .as_deref()
            .and_then(|m| MODULATIONS.iter().position(|x| *x == m))
            .unwrap_or(0);
        let fec_sel = preset
            .fec
            .as_deref()
            .and_then(|f| FECS.iter().position(|x| *x == f))
            .unwrap_or(0);
        Self {
            label: preset.label.clone(),
            frequency_mhz: preset.frequency_mhz,
            delivery_sel,
            bandwidth_sel,
            guard_sel,
            plp: preset.plp.unwrap_or(0),
            modulation_sel,
            fec_sel,
            sel: 0,
            editing: None,
            edit_buffer: String::new(),
        }
    }

    pub fn new_default() -> Self {
        Self {
            label: String::new(),
            frequency_mhz: 618,
            delivery_sel: 0,
            bandwidth_sel: 0,
            guard_sel: 0,
            plp: 0,
            modulation_sel: 0,
            fec_sel: 4, // auto (not sent to tsp for terrestrial)
            sel: 0,
            editing: None,
            edit_buffer: String::new(),
        }
    }

    /// Builds a mux descriptor from the current form values.
    pub fn to_mux(&self) -> TerrestrialMux {
        TerrestrialMux {
            label: if self.label.trim().is_empty() {
                format!("{} MHz {}", self.frequency_mhz, DELIVERY[self.delivery_sel])
            } else {
                self.label.trim().to_string()
            },
            location: "manual".into(),
            uhf_channel: None,
            frequency_mhz: self.frequency_mhz,
            delivery_system: DELIVERY[self.delivery_sel].into(),
            bandwidth_mhz: BANDWIDTHS_MHZ[self.bandwidth_sel],
            guard_interval: GUARD_INTERVALS[self.guard_sel].into(),
            modulation: Some(MODULATIONS[self.modulation_sel].into()),
            fec: Some(FECS[self.fec_sel].into()),
            plp: Some(self.plp.min(7)),
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
        matches!(
            row,
            ROW_DELIVERY | ROW_BANDWIDTH | ROW_GUARD | ROW_PLP | ROW_MODULATION | ROW_FEC
        )
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
                ROW_LABEL => {
                    self.begin_edit(EditField::Label, self.label.clone());
                }
                ROW_FREQUENCY => {
                    self.begin_edit(EditField::Frequency, self.frequency_mhz.to_string())
                }
                ROW_START => return Some(self.validate_and_start()),
                _ => {}
            },
            _ => {}
        }
        None
    }

    fn validate_and_start(&self) -> Result<(), &'static str> {
        if self.frequency_mhz < 47 || self.frequency_mhz > 862 {
            return Err("frequency must be 47–862 MHz (VHF/UHF)");
        }
        Ok(())
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
                EditField::Label if c.is_ascii_graphic() || c == ' ' => {
                    if self.edit_buffer.len() < 48 {
                        self.edit_buffer.push(c);
                    }
                    EditResult::Continue
                }
                EditField::Frequency if c.is_ascii_digit() => {
                    if self.edit_buffer.len() < 4 {
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
            EditField::Label => self.label = self.edit_buffer.trim().to_string(),
            EditField::Frequency => {
                if let Ok(v) = self.edit_buffer.parse::<u32>() {
                    self.frequency_mhz = v.clamp(47, 862);
                }
            }
        }
        self.cancel_edit();
        EditResult::Committed
    }

    fn adjust_choice(&mut self, delta: i32) {
        match self.sel {
            ROW_DELIVERY => Self::cycle_usize(&mut self.delivery_sel, DELIVERY.len(), delta),
            ROW_BANDWIDTH => Self::cycle_usize(&mut self.bandwidth_sel, BANDWIDTHS_MHZ.len(), delta),
            ROW_GUARD => Self::cycle_usize(&mut self.guard_sel, GUARD_INTERVALS.len(), delta),
            ROW_PLP => {
                let next = (self.plp as i32 + delta).rem_euclid(8) as u8;
                self.plp = next;
            }
            ROW_MODULATION => Self::cycle_usize(&mut self.modulation_sel, MODULATIONS.len(), delta),
            ROW_FEC => Self::cycle_usize(&mut self.fec_sel, FECS.len(), delta),
            _ => {}
        }
    }

    fn cycle_usize(value: &mut usize, len: usize, delta: i32) {
        if len == 0 {
            return;
        }
        let next = (*value as i32 + delta).rem_euclid(len as i32) as usize;
        *value = next;
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
            format!("Delivery: {}", DELIVERY[self.delivery_sel]),
            format!("Bandwidth: {} MHz", BANDWIDTHS_MHZ[self.bandwidth_sel]),
            format!("Guard interval: {}", GUARD_INTERVALS[self.guard_sel]),
            format!("PLP (0–7): {}", self.plp),
            format!("Modulation: {}", MODULATIONS[self.modulation_sel]),
            format!("FEC: {}", FECS[self.fec_sel]),
            "▶ Start capture".into(),
        ]
    }

    fn key_hint(&self) -> &'static str {
        match self.editing {
            Some(EditField::Label) => "Type label · Enter done · Esc cancel",
            Some(EditField::Frequency) => "Type MHz · Enter done · Esc cancel",
            None if Self::is_choice_row(self.sel) => "←→ change value",
            None => "↑↓ select · Enter edit/start · choice: ←→ · Esc back",
        }
    }
}

enum EditResult {
    Continue,
    Committed,
}

/// Renders the manual terrestrial form.
pub fn draw_manual_terrestrial(
    frame: &mut Frame,
    form: &ManualTerrestrialForm,
    status: &str,
    device_hint: &str,
) {
    let area = frame.area();
    let footer_h = KEYS_FOOTER_HEIGHT + 2;
    let list_h = area.height.saturating_sub(footer_h);
    let list_area = Rect::new(area.x, area.y, area.width, list_h);
    let footer = Rect::new(area.x, area.y + list_h, area.width, footer_h);

    list_menu(
        frame,
        list_area,
        "Manual terrestrial setup",
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrestrial::turkey_presets;

    #[test]
    fn from_preset_round_trip() {
        let p = &turkey_presets()[0];
        let form = ManualTerrestrialForm::from_preset(p);
        let mux = form.to_mux();
        assert_eq!(mux.frequency_mhz, p.frequency_mhz);
        assert_eq!(mux.delivery_system, p.delivery_system);
    }
}

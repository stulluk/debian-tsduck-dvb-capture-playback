//! Persistent capture settings editor for the TUI.

use crate::config::{default_capture_dir, AppConfig};
use crate::tui::widgets::{list_menu, paragraph};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;

const MIN_SECONDS: u32 = 1;
const MAX_SECONDS: u32 = 600;

/// Editable capture settings form (persisted to `config.json`).
pub struct CaptureSettingsForm {
    pub capture_dir: String,
    pub capture_seconds: u32,
    pub sel: usize,
    pub editing_dir: bool,
    pub status: String,
}

impl CaptureSettingsForm {
    /// Loads current values from disk config.
    pub fn from_config(cfg: &AppConfig) -> Self {
        Self {
            capture_dir: cfg.capture_dir.to_string_lossy().into_owned(),
            capture_seconds: cfg.capture_seconds,
            sel: 0,
            editing_dir: false,
            status: String::new(),
        }
    }

    /// Writes form values into an in-memory config (does not save).
    pub fn apply_to(&self, cfg: &mut AppConfig) {
        cfg.capture_dir = AppConfig::expand_path(&self.capture_dir);
        cfg.capture_seconds = self.capture_seconds.clamp(MIN_SECONDS, MAX_SECONDS);
    }

    /// Persists settings to `~/.config/dvbhub/config.json`.
    pub fn save(&self) -> anyhow::Result<()> {
        let mut cfg = AppConfig::load();
        self.apply_to(&mut cfg);
        cfg.save()?;
        Ok(())
    }

    fn rows(&self) -> Vec<String> {
        vec![
            format!("Capture directory: {}", self.capture_dir),
            format!("Seconds per TP: {}", self.capture_seconds),
            "Reset to defaults".into(),
            "Save and return".into(),
        ]
    }

    /// Draws the settings panel.
    pub fn draw(&self, frame: &mut Frame) {
        let area = frame.area();
        let body = format!(
            "Saved to ~/.config/dvbhub/config.json\n\
             Satellite subfolder is appended automatically (e.g. …/turksat42e/).\n\n\
             {}",
            if self.status.is_empty() {
                if self.editing_dir {
                    "Editing path — type, Backspace, Enter when done"
                } else {
                    ""
                }
            } else {
                &self.status
            }
        );
        paragraph(frame, area, "Capture settings", &body);
        let list_h = area.height.saturating_sub(8).max(6);
        let list_area = Rect {
            x: area.x + 1,
            y: area.y + 6,
            width: area.width.saturating_sub(2),
            height: list_h,
        };
        list_menu(frame, list_area, "Fields", &self.rows(), self.sel);
        paragraph(
            frame,
            Rect {
                x: area.x,
                y: area.y + area.height.saturating_sub(3),
                width: area.width,
                height: 3,
            },
            "Keys",
            if self.editing_dir {
                "Enter finish edit · Esc cancel edit · type path · ~ = home"
            } else {
                "↑↓ select · ←→ ±1s · +/- ±5s · Enter edit/save · Esc back · r reset row"
            },
        );
    }

    /// Handles a key; returns `Some(true)` to save+close, `Some(false)` to close without save.
    pub fn on_key(&mut self, key: KeyEvent) -> Option<bool> {
        if self.editing_dir {
            return self.on_key_editing_dir(key);
        }

        match key.code {
            KeyCode::Esc => return Some(false),
            KeyCode::Up if self.sel > 0 => self.sel -= 1,
            KeyCode::Down if self.sel + 1 < 4 => self.sel += 1,
            KeyCode::Left if self.sel == 1 => self.bump_seconds(-1),
            KeyCode::Right if self.sel == 1 => self.bump_seconds(1),
            KeyCode::Char('-') | KeyCode::Char('_') if self.sel == 1 => self.bump_seconds(-5),
            KeyCode::Char('+') | KeyCode::Char('=') if self.sel == 1 => self.bump_seconds(5),
            KeyCode::Char('r') | KeyCode::Char('R') => self.reset_row(),
            KeyCode::Enter => match self.sel {
                0 => {
                    self.editing_dir = true;
                    self.status.clear();
                }
                1 => {}
                2 => self.reset_all(),
                3 => return Some(true),
                _ => {}
            },
            _ => {}
        }
        None
    }

    fn on_key_editing_dir(&mut self, key: KeyEvent) -> Option<bool> {
        match key.code {
            KeyCode::Esc => {
                self.editing_dir = false;
                self.status.clear();
            }
            KeyCode::Enter => {
                self.editing_dir = false;
                self.capture_dir = AppConfig::expand_path(&self.capture_dir)
                    .to_string_lossy()
                    .into_owned();
                self.status = "Path updated (not saved until Save)".into();
            }
            KeyCode::Backspace => {
                self.capture_dir.pop();
            }
            KeyCode::Char('~') => {
                if self.capture_dir.is_empty() {
                    if let Some(home) = crate::config::home_dir() {
                        self.capture_dir = home.to_string_lossy().into_owned();
                    } else {
                        self.capture_dir.push('~');
                    }
                } else {
                    self.capture_dir.push('~');
                }
            }
            KeyCode::Char(c) => self.capture_dir.push(c),
            _ => {}
        }
        None
    }

    fn bump_seconds(&mut self, delta: i32) {
        let next = (self.capture_seconds as i32 + delta).clamp(MIN_SECONDS as i32, MAX_SECONDS as i32);
        self.capture_seconds = next as u32;
    }

    fn reset_row(&mut self) {
        match self.sel {
            0 => {
                self.capture_dir = default_capture_dir().to_string_lossy().into_owned();
            }
            1 => self.capture_seconds = AppConfig::default().capture_seconds,
            _ => {}
        }
        self.status = "Row reset (not saved until Save)".into();
    }

    fn reset_all(&mut self) {
        let def = AppConfig::default();
        self.capture_dir = def.capture_dir.to_string_lossy().into_owned();
        self.capture_seconds = def.capture_seconds;
        self.status = "Defaults restored (not saved until Save)".into();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_clamps_seconds() {
        let mut form = CaptureSettingsForm::from_config(&AppConfig::default());
        form.capture_seconds = 9999;
        let mut cfg = AppConfig::default();
        form.apply_to(&mut cfg);
        assert_eq!(cfg.capture_seconds, MAX_SECONDS);
    }
}

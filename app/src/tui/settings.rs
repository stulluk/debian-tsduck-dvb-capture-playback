//! Persistent capture settings editor for the TUI.

use crate::config::{default_capture_dir, AppConfig};
use crate::tui::widgets::{list_menu, paragraph};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use std::time::{Duration, Instant};

const MIN_SECONDS: u32 = 1;
const MAX_SECONDS: u32 = 3600;

/// Two arrow events closer than this belong to one held key (terminal auto-repeat).
const HOLD_GAP: Duration = Duration::from_millis(200);
/// Held longer than this: step 10 s.
const HOLD_STEP10_AFTER: Duration = Duration::from_millis(500);
/// Held longer than this: step 60 s.
const HOLD_STEP60_AFTER: Duration = Duration::from_millis(2000);

/// Detects a held ←/→ key from auto-repeat timing and picks the step size.
///
/// Most terminals send no key-release events, so a hold is recognised as a run
/// of same-direction presses less than [`HOLD_GAP`] apart. A single press steps
/// 1 s; holding steps 1 s, then 10 s, then 60 s.
#[derive(Debug, Default)]
struct HoldAccel {
    dir: i32,
    hold_start: Option<Instant>,
    last: Option<Instant>,
}

impl HoldAccel {
    /// Returns the step (1, 10 or 60) for a press in direction `dir` at `now`.
    fn step(&mut self, dir: i32, now: Instant) -> u32 {
        let held = self.dir == dir
            && self
                .last
                .is_some_and(|last| now.saturating_duration_since(last) <= HOLD_GAP);
        if !held {
            self.dir = dir;
            self.hold_start = Some(now);
        }
        self.last = Some(now);
        let held_for = now.saturating_duration_since(self.hold_start.unwrap_or(now));
        if held_for >= HOLD_STEP60_AFTER {
            60
        } else if held_for >= HOLD_STEP10_AFTER {
            10
        } else {
            1
        }
    }
}

/// Moves `value` one `step` in direction `dir`, snapping to multiples of `step`
/// (e.g. 37 → 40 → 50 with step 10) and clamping to the allowed range.
fn stepped_seconds(value: u32, dir: i32, step: u32) -> u32 {
    let next = if dir > 0 {
        (value / step + 1) * step
    } else if value % step == 0 {
        value.saturating_sub(step)
    } else {
        value - value % step
    };
    next.clamp(MIN_SECONDS, MAX_SECONDS)
}

/// Editable capture settings form (persisted to `config.json`).
pub struct CaptureSettingsForm {
    pub capture_dir: String,
    pub capture_seconds: u32,
    pub sel: usize,
    pub editing_dir: bool,
    pub status: String,
    hold: HoldAccel,
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
            hold: HoldAccel::default(),
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
                "↑↓ select · ←→ ±1s (hold: 10s, then 60s) · +/- ±5s · Enter edit/save · Esc back · r reset row"
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
            KeyCode::Left if self.sel == 1 => self.arrow_seconds(-1, Instant::now()),
            KeyCode::Right if self.sel == 1 => self.arrow_seconds(1, Instant::now()),
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

    /// ←/→ handler: 1 s per press, accelerating while the key is held.
    fn arrow_seconds(&mut self, dir: i32, now: Instant) {
        let step = self.hold.step(dir, now);
        self.capture_seconds = stepped_seconds(self.capture_seconds, dir, step);
    }

    fn bump_seconds(&mut self, delta: i32) {
        let next =
            (self.capture_seconds as i32 + delta).clamp(MIN_SECONDS as i32, MAX_SECONDS as i32);
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

    /// Simulates a key held for `ms` with auto-repeat every `period_ms`.
    fn hold(form: &mut CaptureSettingsForm, dir: i32, t0: Instant, ms: u64, period_ms: u64) {
        let mut t = 0;
        while t <= ms {
            form.arrow_seconds(dir, t0 + Duration::from_millis(t));
            t += period_ms;
        }
    }

    #[test]
    fn single_presses_step_one_second() {
        let mut form = CaptureSettingsForm::from_config(&AppConfig::default());
        form.capture_seconds = 30;
        let t0 = Instant::now();
        form.arrow_seconds(1, t0);
        form.arrow_seconds(1, t0 + Duration::from_millis(400));
        form.arrow_seconds(1, t0 + Duration::from_millis(800));
        assert_eq!(form.capture_seconds, 33);
        form.arrow_seconds(-1, t0 + Duration::from_millis(1200));
        assert_eq!(form.capture_seconds, 32);
    }

    #[test]
    fn short_hold_stays_on_one_second_steps() {
        let mut form = CaptureSettingsForm::from_config(&AppConfig::default());
        form.capture_seconds = 30;
        hold(&mut form, 1, Instant::now(), 400, 40); // 11 events < 500 ms
        assert_eq!(form.capture_seconds, 41);
    }

    #[test]
    fn hold_accelerates_to_ten_then_sixty() {
        let mut form = CaptureSettingsForm::from_config(&AppConfig::default());
        form.capture_seconds = 30;
        let t0 = Instant::now();
        hold(&mut form, 1, t0, 1000, 40);
        let after_1s = form.capture_seconds;
        assert_eq!(after_1s % 10, 0, "snapped to 10 s: {after_1s}");
        assert!(after_1s > 100 && after_1s < 300, "{after_1s}");
        hold(&mut form, 1, t0 + Duration::from_millis(1040), 1400, 40);
        assert_eq!(form.capture_seconds % 60, 0, "snapped to 60 s");
        assert!(form.capture_seconds > after_1s + 60);
    }

    #[test]
    fn hold_is_capped_at_one_hour_and_one_second() {
        let mut form = CaptureSettingsForm::from_config(&AppConfig::default());
        form.capture_seconds = 30;
        let t0 = Instant::now();
        hold(&mut form, 1, t0, 10_000, 30);
        assert_eq!(form.capture_seconds, 3600);
        hold(
            &mut form,
            -1,
            t0 + Duration::from_millis(10_500),
            10_000,
            30,
        );
        assert_eq!(form.capture_seconds, 1);
    }

    #[test]
    fn direction_change_resets_acceleration() {
        let mut form = CaptureSettingsForm::from_config(&AppConfig::default());
        form.capture_seconds = 300;
        let t0 = Instant::now();
        hold(&mut form, 1, t0, 3000, 40);
        let v = form.capture_seconds;
        form.arrow_seconds(-1, t0 + Duration::from_millis(3040));
        assert_eq!(form.capture_seconds, v - 1);
    }

    #[test]
    fn stepped_seconds_snaps_and_clamps() {
        assert_eq!(stepped_seconds(37, 1, 10), 40);
        assert_eq!(stepped_seconds(40, 1, 10), 50);
        assert_eq!(stepped_seconds(37, -1, 10), 30);
        assert_eq!(stepped_seconds(30, -1, 10), 20);
        assert_eq!(stepped_seconds(3590, 1, 60), 3600);
        assert_eq!(stepped_seconds(3600, 1, 60), 3600);
        assert_eq!(stepped_seconds(5, -1, 10), 1);
    }
}

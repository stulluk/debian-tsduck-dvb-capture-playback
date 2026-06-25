//! Yes/No confirmation overlay for destructive actions (stop capture/playout).

use crate::tui::widgets::paragraph;
use crossterm::event::KeyCode;
use ratatui::prelude::*;

/// Blocking Y/N prompt drawn over the current screen.
#[derive(Debug, Clone, Default)]
pub struct ConfirmPrompt {
    pub active: bool,
    pub message: String,
}

impl ConfirmPrompt {
    /// Shows the prompt with the given message.
    pub fn open(message: impl Into<String>) -> Self {
        Self {
            active: true,
            message: message.into(),
        }
    }

    /// Activates the prompt on an existing value.
    pub fn ask(&mut self, message: impl Into<String>) {
        *self = Self::open(message);
    }

    /// Clears the prompt without choosing.
    pub fn dismiss(&mut self) {
        self.active = false;
        self.message.clear();
    }

    /// Draws a centered confirmation box when active.
    pub fn draw(&self, frame: &mut Frame) {
        if !self.active {
            return;
        }
        let area = frame.area();
        let w = area.width.min(72).max(24);
        let h = 7u16.min(area.height);
        let x = area.x + (area.width.saturating_sub(w)) / 2;
        let y = area.y + (area.height.saturating_sub(h)) / 2;
        let popup = Rect::new(x, y, w, h);
        let body = format!("{}\n\nY yes · N no · Esc cancel", self.message);
        paragraph(frame, popup, "Confirm", &body);
    }

    /// While active: `Some(true)` yes, `Some(false)` no, `None` still waiting / dismissed.
    pub fn on_key(&mut self, code: KeyCode) -> Option<bool> {
        if !self.active {
            return None;
        }
        match code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.dismiss();
                Some(true)
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.dismiss();
                Some(false)
            }
            _ => None,
        }
    }
}

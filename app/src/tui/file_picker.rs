//! Simple directory navigator for choosing a `.ts` file in the TUI.

use crate::config::{default_capture_dir, home_dir};
use crate::tui::widgets::{list_menu, paragraph, picker_layout};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use std::path::{Path, PathBuf};

/// One row in the file picker list.
#[derive(Debug, Clone)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
}

/// Navigates directories and selects a transport-stream file.
pub struct FilePicker {
    pub current_dir: PathBuf,
    entries: Vec<DirEntry>,
    pub selected: usize,
    pub status: String,
}

impl FilePicker {
    /// Opens the picker at `start` (or `~/dvbhub-captures` when missing).
    pub fn open(start: Option<PathBuf>) -> Self {
        let mut picker = Self {
            current_dir: start.unwrap_or_else(default_capture_dir),
            entries: Vec::new(),
            selected: 0,
            status: String::new(),
        };
        picker.refresh();
        picker
    }

    /// Reloads directory entries (`..`, subdirs, `.ts` files).
    pub fn refresh(&mut self) {
        self.entries.clear();
        if self.current_dir.parent().is_some() {
            self.entries.push(DirEntry {
                name: "..".into(),
                is_dir: true,
            });
        }
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        if let Ok(read) = std::fs::read_dir(&self.current_dir) {
            for entry in read.filter_map(Result::ok) {
                let Ok(ft) = entry.file_type() else {
                    continue;
                };
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                if ft.is_dir() {
                    dirs.push(name);
                } else if ft.is_file() && name.to_ascii_lowercase().ends_with(".ts") {
                    files.push(name);
                }
            }
        } else {
            self.status = format!("cannot read {}", self.current_dir.display());
        }
        dirs.sort();
        files.sort();
        for name in dirs {
            self.entries.push(DirEntry {
                name,
                is_dir: true,
            });
        }
        for name in files {
            self.entries.push(DirEntry {
                name,
                is_dir: false,
            });
        }
        if self.selected >= self.entries.len() {
            self.selected = self.entries.len().saturating_sub(1);
        }
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    fn labels(&self) -> Vec<String> {
        self.entries
            .iter()
            .map(|e| {
                if e.is_dir {
                    format!("{}/", e.name)
                } else {
                    e.name.clone()
                }
            })
            .collect()
    }

    /// Draws the navigator (path bar + list + keys).
    pub fn draw(&self, frame: &mut Frame) {
        let [list_area, path_area, keys_area] = picker_layout(frame.area());
        list_menu(
            frame,
            list_area,
            "Select transport stream (.ts)",
            &self.labels(),
            self.selected,
        );
        let path_line = format!("{}", self.current_dir.display());
        paragraph(frame, path_area, "Directory", &path_line);
        let hint = if self.status.is_empty() {
            "↑↓ PgUp/PgDn · Enter · Backspace parent · ~ home · Esc back · q quit".into()
        } else {
            format!("{} · q quit", self.status)
        };
        paragraph(frame, keys_area, "Keys", &hint);
    }

    /// Handles a key press. Returns `Some(path)` when a `.ts` file is chosen.
    pub fn on_key(&mut self, key: KeyEvent) -> Option<PathBuf> {
        match key.code {
            KeyCode::Up if self.selected > 0 => self.selected -= 1,
            KeyCode::Down if self.selected + 1 < self.entries.len() => self.selected += 1,
            KeyCode::PageUp => {
                let step = 10usize;
                self.selected = self.selected.saturating_sub(step);
            }
            KeyCode::PageDown => {
                let max = self.entries.len().saturating_sub(1);
                self.selected = (self.selected + 10).min(max);
            }
            KeyCode::Backspace | KeyCode::Esc => self.go_parent(),
            KeyCode::Char('~') => {
                if let Some(home) = home_dir() {
                    self.enter_dir(&home);
                }
            }
            KeyCode::Enter => {
                let entry = self.entries.get(self.selected)?.clone();
                if entry.is_dir {
                    if entry.name == ".." {
                        self.go_parent();
                    } else {
                        self.enter_dir(&self.current_dir.join(&entry.name));
                    }
                    return None;
                }
                return Some(self.current_dir.join(&entry.name));
            }
            _ => {}
        }
        None
    }

    fn enter_dir(&mut self, path: &Path) {
        if path.is_dir() {
            self.current_dir = path.to_path_buf();
            self.selected = 0;
            self.status.clear();
            self.refresh();
        }
    }

    fn go_parent(&mut self) {
        let parent = self.current_dir.parent().map(Path::to_path_buf);
        if let Some(parent) = parent {
            self.enter_dir(&parent);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_starts_with_parent_entry_when_nested() {
        let picker = FilePicker::open(Some(PathBuf::from("/tmp")));
        assert!(picker.entries.iter().any(|e| e.name == ".."));
    }
}

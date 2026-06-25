//! Terminal UI (ratatui) for capture and playout workflows.

mod capture;
mod confirm;
mod file_picker;
mod lnb_presets;
mod manual_capture;
mod playout;
mod settings;
mod widgets;

pub use capture::run_capture_tui;
pub use playout::run_playout_tui;

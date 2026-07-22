//! Terminal UI (ratatui) for capture and playout workflows.

mod capture;
mod confirm;
mod file_picker;
mod lnb_presets;
mod manual_cable;
mod manual_capture;
mod manual_playout_cable;
mod manual_playout_satellite;
mod manual_playout_terrestrial;
mod manual_terrestrial;
mod playout;
mod settings;
mod watch;
mod widgets;

pub use capture::run_capture_tui;
pub use playout::run_playout_tui;
pub use watch::run_watch_tui;

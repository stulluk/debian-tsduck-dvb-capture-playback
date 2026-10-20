//! Shared ratatui drawing helpers.

use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, StatefulWidget},
    Frame,
};

/// Height reserved for the bordered keys/help footer (top + text + bottom).
pub const KEYS_FOOTER_HEIGHT: u16 = 3;

/// Height reserved for a one-line path / status bar above the keys footer.
pub const INFO_FOOTER_HEIGHT: u16 = 3;

/// Renders a titled list menu; `selected` is the highlighted row index.
/// Long lists scroll automatically so the selection stays visible.
pub fn list_menu(frame: &mut Frame, area: Rect, title: &str, items: &[String], selected: usize) {
    if area.height < 3 || area.width < 3 {
        return;
    }

    if items.is_empty() {
        let list = List::new(Vec::<ListItem>::new())
            .block(Block::default().borders(Borders::ALL).title(title));
        frame.render_widget(list, area);
        return;
    }

    let selected = selected.min(items.len() - 1);
    let max_label = area.width.saturating_sub(4) as usize;
    let rows: Vec<ListItem> = items
        .iter()
        .map(|label| ListItem::new(Line::from(truncate_tail(label, max_label))))
        .collect();

    let list = List::new(rows)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

    let mut state = ListState::default().with_selected(Some(selected));
    StatefulWidget::render(list, area, frame.buffer_mut(), &mut state);
}

/// Height reserved for the bordered "last run" summary footer.
pub const SUMMARY_FOOTER_HEIGHT: u16 = 3;

/// Vertical split for wizard list screens: scrollable list + optional summary + keys.
pub struct WizardListLayout {
    pub list: Rect,
    pub summary: Option<Rect>,
    pub keys: Rect,
}

/// Splits `area` into list / optional summary / keys regions (non-overlapping).
pub fn wizard_list_layout(area: Rect, show_summary: bool) -> WizardListLayout {
    if show_summary {
        let chunks = Layout::vertical([
            Constraint::Min(4),
            Constraint::Length(SUMMARY_FOOTER_HEIGHT),
            Constraint::Length(KEYS_FOOTER_HEIGHT),
        ])
        .split(area);
        WizardListLayout {
            list: chunks[0],
            summary: Some(chunks[1]),
            keys: chunks[2],
        }
    } else {
        let chunks = Layout::vertical([Constraint::Min(4), Constraint::Length(KEYS_FOOTER_HEIGHT)])
            .split(area);
        WizardListLayout {
            list: chunks[0],
            summary: None,
            keys: chunks[1],
        }
    }
}

/// Total footer height for PageUp/PageDown step sizing.
pub fn wizard_footer_rows(show_summary: bool) -> u16 {
    if show_summary {
        SUMMARY_FOOTER_HEIGHT + KEYS_FOOTER_HEIGHT
    } else {
        KEYS_FOOTER_HEIGHT
    }
}

/// Rows visible in a list (for PageUp/PageDown step size).
pub fn list_viewport_rows(total_height: u16, footer_rows: u16) -> usize {
    total_height
        .saturating_sub(2)
        .saturating_sub(footer_rows)
        .max(1) as usize
}

/// Renders a bordered paragraph (status / help text), clipped to the area.
pub fn paragraph(frame: &mut Frame, area: Rect, title: &str, body: &str) {
    if area.height < 2 || area.width < 2 {
        return;
    }
    let max_lines = area.height.saturating_sub(2) as usize;
    let max_width = area.width.saturating_sub(2) as usize;
    let clipped = clip_body(body, max_width, max_lines);
    let p = Paragraph::new(clipped).block(Block::default().borders(Borders::ALL).title(title));
    frame.render_widget(p, area);
}

fn clip_body(text: &str, max_width: usize, max_lines: usize) -> String {
    if max_lines == 0 || max_width == 0 {
        return String::new();
    }
    let mut lines_out = Vec::new();
    for line in text.lines() {
        if lines_out.len() >= max_lines {
            break;
        }
        if line.chars().count() <= max_width {
            lines_out.push(line.to_string());
        } else {
            lines_out.push(truncate_tail(line, max_width));
        }
    }
    lines_out.join("\n")
}

/// Shortens `s` to `max_chars`, keeping the start (labels, frequencies) and
/// marking the cut with a trailing `…`.
pub fn truncate_tail(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        return s.to_string();
    }
    if max_chars == 0 {
        return String::new();
    }
    let head: String = s.chars().take(max_chars - 1).collect();
    format!("{head}…")
}

/// Shortens `s` to `max_chars`, keeping the end (useful for file paths) and
/// marking the cut with a leading `…`.
pub fn truncate_end(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let count = s.chars().count();
    if count <= max_chars {
        return s.to_string();
    }
    if max_chars <= 1 {
        return "…".to_string();
    }
    let tail: String = s.chars().skip(count - (max_chars - 1)).collect();
    format!("…{tail}")
}

/// Vertical split for file-picker style screens: list / info bar / keys.
pub fn picker_layout(area: Rect) -> [Rect; 3] {
    let chunks = Layout::vertical([
        Constraint::Min(4),
        Constraint::Length(INFO_FOOTER_HEIGHT),
        Constraint::Length(KEYS_FOOTER_HEIGHT),
    ])
    .split(area);
    [chunks[0], chunks[1], chunks[2]]
}

/// Standard capture run layout: TP queue | services | signal | status | log.
pub fn capture_run_layout(area: Rect) -> [Rect; 5] {
    let chunks = ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            Constraint::Percentage(42),
            Constraint::Length(4),
            Constraint::Min(8),
        ])
        .split(area);
    let top = ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Horizontal)
        .constraints([
            Constraint::Percentage(34),
            Constraint::Min(20),
            // Fixed width so "Bitrate: 123.45 Mbps" fits on one line.
            Constraint::Length(22),
        ])
        .split(chunks[0]);
    [top[0], top[1], top[2], chunks[1], chunks[2]]
}

/// Renders a scrollable log (shows the last lines that fit).
pub fn log_panel(frame: &mut Frame, area: Rect, title: &str, lines: &[String]) {
    let height = area.height.saturating_sub(2) as usize;
    let visible: Vec<ListItem> = lines
        .iter()
        .rev()
        .take(height)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|line| ListItem::new(Line::from(line.as_str())))
        .collect();
    let list = List::new(visible).block(Block::default().borders(Borders::ALL).title(title));
    frame.render_widget(list, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn list_menu_renders_selection() {
        let backend = TestBackend::new(60, 12);
        let mut term = Terminal::new(backend).unwrap();
        term.backend_mut().resize(60, 12);
        term.draw(|f| {
            list_menu(
                f,
                f.area(),
                "Devices",
                &["/dev/dvb/adapter0 PCTV 461".into(), "adapter1".into()],
                0,
            );
        })
        .unwrap();
        let buf = term.backend().buffer();
        let text = format!("{buf:?}");
        assert!(text.contains("Devices"));
        assert!(text.contains("adapter0"));
    }

    #[test]
    fn wizard_list_layout_reserves_keys_footer() {
        let area = Rect::new(0, 0, 80, 20);
        let layout = wizard_list_layout(area, false);
        assert_eq!(layout.keys.height, KEYS_FOOTER_HEIGHT);
        assert_eq!(layout.keys.y + layout.keys.height, area.height);
        assert!(layout.list.height + layout.keys.height <= area.height);
    }

    #[test]
    fn list_viewport_rows_accounts_for_footer() {
        assert_eq!(list_viewport_rows(24, 4), 18);
        assert_eq!(list_viewport_rows(10, 20), 1);
    }

    #[test]
    fn list_menu_scrolls_long_lists() {
        let backend = TestBackend::new(60, 10);
        let mut term = Terminal::new(backend).unwrap();
        let items: Vec<String> = (0..50).map(|i| format!("TP #{i}")).collect();
        term.draw(|f| list_menu(f, f.area(), "Transponders", &items, 40))
            .unwrap();
        let buf = term.backend().buffer();
        let text = format!("{buf:?}");
        assert!(text.contains("TP #40"));
        assert!(!text.contains("TP #0"));
    }

    #[test]
    fn list_menu_keeps_selected_index() {
        let backend = TestBackend::new(60, 10);
        let mut term = Terminal::new(backend).unwrap();
        let items: Vec<String> = (0..50).map(|i| format!("TP #{i}")).collect();
        term.draw(|f| list_menu(f, f.area(), "Transponders", &items, 8))
            .unwrap();
        let buf = term.backend().buffer();
        let text = format!("{buf:?}");
        assert!(text.contains("TP #8"));
        assert!(!text.contains("TP #9"));
    }
}

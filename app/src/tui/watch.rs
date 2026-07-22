//! Interactive watch TUI: delivery filter → channel list → mpv/vlc playback.

use crate::bench;
use crate::channels::{ChannelDb, ChannelDelivery, SavedChannel};
use crate::channels_import::{self, ImportReport};
use crate::config::{AppConfig, TsduckPaths};
use crate::devices::Discovery;
use crate::tui::capture::run_capture_tui;
use crate::tui::widgets::{list_menu, list_viewport_rows, paragraph, KEYS_FOOTER_HEIGHT};
use crate::watch;
use crate::lastlog;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::prelude::*;
use std::io::stdout;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    NoChannels,
    DeliveryPick,
    ChannelList,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListFilter {
    Satellite,
    Terrestrial,
    Cable,
    All,
}

impl ListFilter {
    fn label(self, db: &ChannelDb) -> String {
        match self {
            Self::Satellite => format!("Satellite ({})", db.count_for(&ChannelDelivery::Satellite)),
            Self::Terrestrial => {
                format!("Terrestrial ({})", db.count_for(&ChannelDelivery::Terrestrial))
            }
            Self::Cable => format!("Cable ({})", db.count_for(&ChannelDelivery::Cable)),
            Self::All => format!("All channels ({})", db.channels.len()),
        }
    }

    fn delivery(self) -> Option<ChannelDelivery> {
        match self {
            Self::Satellite => Some(ChannelDelivery::Satellite),
            Self::Terrestrial => Some(ChannelDelivery::Terrestrial),
            Self::Cable => Some(ChannelDelivery::Cable),
            Self::All => None,
        }
    }
}

struct WatchState {
    db: ChannelDb,
    discovery: Discovery,
    paths: TsduckPaths,
    cfg: AppConfig,
    screen: Screen,
    delivery_options: Vec<ListFilter>,
    sel_delivery: usize,
    active_filter: ListFilter,
    channel_ids: Vec<u64>,
    sel_channel: usize,
    status: String,
    last_import: ImportReport,
}

/// Runs `dvbhub watch --tui`.
pub fn run_watch_tui(paths: TsduckPaths) -> anyhow::Result<()> {
    lastlog::event("tui.watch", "start");
    let discovery = crate::devices::discover(&paths)?;
    if !discovery.has_capture_tuners() {
        lastlog::event("tui.watch", "abort: no capture tuners");
        anyhow::bail!("no DVB capture tuner found — connect PCTV 461, TBS5530, or similar");
    }
    lastlog::event(
        "tui.watch",
        &format!(
            "tuners={} warnings={}",
            discovery.capture_tuners().len(),
            discovery.warnings.len()
        ),
    );

    let cfg = AppConfig::load();
    let mut db = ChannelDb::load();
    let _ = bench::ensure_bench_sidecars(&cfg.capture_dir);
    let mut last_import = ImportReport::default();
    if db.channels.is_empty() {
        last_import =
            channels_import::import_from_capture_dir(&cfg.capture_dir, &mut db, Some(&discovery));
        if last_import.channels_added > 0 {
            let _ = db.save();
        }
    }
    lastlog::event(
        "tui.watch",
        &format!(
            "channels={} import_added={} capture_dir={}",
            db.channels.len(),
            last_import.channels_added,
            cfg.capture_dir.display()
        ),
    );

    let mut state = WatchState {
        screen: if db.channels.is_empty() {
            Screen::NoChannels
        } else {
            Screen::DeliveryPick
        },
        delivery_options: build_delivery_options(&db),
        sel_delivery: 0,
        active_filter: ListFilter::All,
        channel_ids: Vec::new(),
        sel_channel: 0,
        status: status_from_import(&last_import, &cfg),
        last_import,
        db,
        discovery,
        paths,
        cfg,
    };

    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout());
    let mut term = Terminal::new(backend)?;

    let result = run_loop(&mut term, &mut state);

    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;
    lastlog::event("tui.watch", "end");
    result
}

fn status_from_import(report: &ImportReport, cfg: &AppConfig) -> String {
    if report.channels_added > 0 {
        format!(
            "Imported {} channel(s) from {} sidecar(s) under {}",
            report.channels_added,
            report.sidecars_read,
            cfg.capture_dir.display()
        )
    } else if report.sidecars_read > 0 {
        format!(
            "No new channels from {} sidecar(s) under {}",
            report.sidecars_read,
            cfg.capture_dir.display()
        )
    } else {
        format!(
            "No *_services.json sidecars under {}",
            cfg.capture_dir.display()
        )
    }
}

fn build_delivery_options(db: &ChannelDb) -> Vec<ListFilter> {
    let mut opts = Vec::new();
    if db.count_for(&ChannelDelivery::Satellite) > 0 {
        opts.push(ListFilter::Satellite);
    }
    if db.count_for(&ChannelDelivery::Terrestrial) > 0 {
        opts.push(ListFilter::Terrestrial);
    }
    if db.count_for(&ChannelDelivery::Cable) > 0 {
        opts.push(ListFilter::Cable);
    }
    if db.channels.len() > 1 {
        opts.push(ListFilter::All);
    }
    if opts.is_empty() {
        opts.push(ListFilter::All);
    }
    opts
}

fn run_loop(
    term: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    state: &mut WatchState,
) -> anyhow::Result<()> {
    loop {
        term.draw(|f| draw(f, state))?;

        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match state.screen {
            Screen::NoChannels => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Char('c') | KeyCode::Char('C') => {
                    open_capture_tui(term, state)?;
                }
                KeyCode::Char('i') | KeyCode::Char('I') => {
                    state.last_import = channels_import::import_from_capture_dir(
                        &state.cfg.capture_dir,
                        &mut state.db,
                        Some(&state.discovery),
                    );
                    state.status = status_from_import(&state.last_import, &state.cfg);
                    if state.last_import.channels_added > 0 {
                        let _ = state.db.save();
                    }
                    if !state.db.channels.is_empty() {
                        state.delivery_options = build_delivery_options(&state.db);
                        state.sel_delivery = 0;
                        state.screen = Screen::DeliveryPick;
                    }
                }
                _ => {}
            },
            Screen::DeliveryPick => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Up | KeyCode::Char('k') => {
                    state.sel_delivery = state.sel_delivery.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let max = state.delivery_options.len().saturating_sub(1);
                    if state.sel_delivery + 1 <= max {
                        state.sel_delivery += 1;
                    }
                }
                KeyCode::PageUp => {
                    let page = page_rows(term);
                    state.sel_delivery = state.sel_delivery.saturating_sub(page);
                }
                KeyCode::PageDown => {
                    let page = page_rows(term);
                    let max = state.delivery_options.len().saturating_sub(1);
                    state.sel_delivery = (state.sel_delivery + page).min(max);
                }
                KeyCode::Enter => {
                    state.active_filter = state.delivery_options[state.sel_delivery];
                    refresh_channel_list(state);
                    if state.channel_ids.is_empty() {
                        state.status = format!(
                            "No {} channels saved — run capture --tui to scan",
                            filter_name(state.active_filter)
                        );
                    }
                    state.screen = Screen::ChannelList;
                    state.sel_channel = 0;
                }
                KeyCode::Char('i') | KeyCode::Char('I') => reimport(state),
                KeyCode::Char('c') | KeyCode::Char('C') => {
                    open_capture_tui(term, state)?;
                }
                _ => {}
            },
            Screen::ChannelList => match key.code {
                KeyCode::Char('q') => break,
                KeyCode::Esc => {
                    state.screen = Screen::DeliveryPick;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    state.sel_channel = state.sel_channel.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let max = state.channel_ids.len().saturating_sub(1);
                    if state.sel_channel + 1 <= max {
                        state.sel_channel += 1;
                    }
                }
                KeyCode::PageUp => {
                    let page = page_rows(term);
                    state.sel_channel = state.sel_channel.saturating_sub(page);
                }
                KeyCode::PageDown => {
                    let page = page_rows(term);
                    let max = state.channel_ids.len().saturating_sub(1);
                    state.sel_channel = (state.sel_channel + page).min(max);
                }
                KeyCode::Enter => {
                    if let Some(ch) = selected_channel(state) {
                        let ch = ch.clone();
                        let paths = state.paths.clone();
                        let discovery = state.discovery.clone();
                        lastlog::event(
                            "tui.watch",
                            &format!(
                                "enter channel={:?} sid={} filter={:?}",
                                ch.name, ch.service_id, state.active_filter
                            ),
                        );
                        lastlog::event("tui.watch", "leave_alt_screen");
                        disable_raw_mode()?;
                        stdout().execute(LeaveAlternateScreen)?;
                        let t0 = std::time::Instant::now();
                        let watch_result =
                            watch::run_watch_for_tui(&paths, &ch, &discovery, false);
                        lastlog::event(
                            "tui.watch",
                            &format!(
                                "returned {:?} elapsed_s={:.2}",
                                watch_result.as_ref().ok(),
                                t0.elapsed().as_secs_f32()
                            ),
                        );
                        enable_raw_mode()?;
                        stdout().execute(EnterAlternateScreen)?;
                        term.clear()?;
                        match watch_result {
                            Ok(watch::WatchTuiOutcome::UserClosedVideo) => {
                                state.status =
                                    "Video closed — pick another channel (↑↓ Enter)".into();
                            }
                            Ok(watch::WatchTuiOutcome::Finished) => {
                                state.status = format!("Finished: {}", ch.name);
                            }
                            Err(e) => state.status = format!("Watch failed: {e:#}"),
                        }
                    }
                }
                KeyCode::Char('i') | KeyCode::Char('I') => {
                    reimport(state);
                    refresh_channel_list(state);
                }
                KeyCode::Char('c') | KeyCode::Char('C') => {
                    open_capture_tui(term, state)?;
                    refresh_channel_list(state);
                }
                _ => {}
            },
        }
    }
    Ok(())
}

fn open_capture_tui(
    term: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    state: &mut WatchState,
) -> anyhow::Result<()> {
    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;
    let paths = state.paths.clone();
    let capture_result = run_capture_tui(paths, false);
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    term.clear()?;
    match capture_result {
        Ok(()) => {
            state.status = "Returned from capture — re-importing sidecars…".into();
            reimport(state);
            if !state.db.channels.is_empty() {
                state.delivery_options = build_delivery_options(&state.db);
                state.screen = Screen::DeliveryPick;
            }
        }
        Err(e) => state.status = format!("Capture TUI: {e:#}"),
    }
    Ok(())
}

fn reimport(state: &mut WatchState) {
    state.last_import = channels_import::import_from_capture_dir(
        &state.cfg.capture_dir,
        &mut state.db,
        Some(&state.discovery),
    );
    state.status = status_from_import(&state.last_import, &state.cfg);
    if state.last_import.channels_added > 0 {
        let _ = state.db.save();
        state.delivery_options = build_delivery_options(&state.db);
    }
}

fn refresh_channel_list(state: &mut WatchState) {
    state.channel_ids = filtered_channels(&state.db, state.active_filter)
        .into_iter()
        .map(|c| c.id)
        .collect();
    if state.sel_channel >= state.channel_ids.len() {
        state.sel_channel = state.channel_ids.len().saturating_sub(1);
    }
}

fn filtered_channels<'a>(db: &'a ChannelDb, filter: ListFilter) -> Vec<&'a SavedChannel> {
    match filter.delivery() {
        Some(d) => db.for_delivery(&d),
        None => db.sorted_all(),
    }
}

fn selected_channel(state: &WatchState) -> Option<&SavedChannel> {
    let id = *state.channel_ids.get(state.sel_channel)?;
    state.db.channels.iter().find(|c| c.id == id)
}

fn filter_name(filter: ListFilter) -> &'static str {
    match filter {
        ListFilter::Satellite => "satellite",
        ListFilter::Terrestrial => "terrestrial",
        ListFilter::Cable => "cable",
        ListFilter::All => "saved",
    }
}

fn page_rows(term: &Terminal<CrosstermBackend<std::io::Stdout>>) -> usize {
    list_viewport_rows(term.size().map(|s| s.height).unwrap_or(24), KEYS_FOOTER_HEIGHT)
}

fn draw(f: &mut Frame, state: &WatchState) {
    let area = f.area();
    match state.screen {
        Screen::NoChannels => draw_no_channels(f, area, state),
        Screen::DeliveryPick => draw_delivery_pick(f, area, state),
        Screen::ChannelList => draw_channel_list(f, area, state),
    }
}

fn draw_no_channels(f: &mut Frame, area: Rect, state: &WatchState) {
    let chunks = Layout::vertical([Constraint::Min(4), Constraint::Length(KEYS_FOOTER_HEIGHT)]).split(area);
    let body = format!(
        "No channels in {}\n\n\
         {}\n\n\
         Capture FTA services with:\n\
           dvbhub capture --tui\n\n\
         Sidecar files (*_services.json) next to .ts captures are imported automatically.",
        crate::config::channels_file_path().display(),
        state.status
    );
    paragraph(f, chunks[0], "dvbhub watch", &body);
    paragraph(
        f,
        chunks[1],
        "keys",
        "i import from captures · c scan (capture --tui) · q quit",
    );
}

fn draw_delivery_pick(f: &mut Frame, area: Rect, state: &WatchState) {
    let chunks = Layout::vertical([Constraint::Min(4), Constraint::Length(KEYS_FOOTER_HEIGHT)]).split(area);
    let items: Vec<String> = state
        .delivery_options
        .iter()
        .map(|f| f.label(&state.db))
        .collect();
    list_menu(
        f,
        chunks[0],
        "Which channels to list?",
        &items,
        state.sel_delivery,
    );
    paragraph(
        f,
        chunks[1],
        "keys",
        &format!(
            "↑↓ PgUp/PgDn · Enter list · i re-import · c scan · q quit · {}",
            state.status
        ),
    );
}

fn draw_channel_list(f: &mut Frame, area: Rect, state: &WatchState) {
    let chunks = Layout::vertical([Constraint::Min(4), Constraint::Length(KEYS_FOOTER_HEIGHT)]).split(area);
    let channels = filtered_channels(&state.db, state.active_filter);
    let items: Vec<String> = channels.iter().map(|c| c.list_label()).collect();
    let title = format!("Watch — {}", state.active_filter.label(&state.db));
    list_menu(f, chunks[0], &title, &items, state.sel_channel);
    paragraph(
        f,
        chunks[1],
        "keys",
        &format!(
            "↑↓ PgUp/PgDn · Enter watch · Esc back · i import · c scan · q quit · {}",
            state.status
        ),
    );
}

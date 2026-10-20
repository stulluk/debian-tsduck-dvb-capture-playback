//! Interactive watch TUI: delivery filter → channel list → mpv/vlc playback.

use crate::bench;
use crate::channels::{ChannelDb, ChannelDelivery, SavedChannel};
use crate::channels_import::{self, ImportReport};
use crate::config::{AppConfig, TsduckPaths};
use crate::devices::Discovery;
use crate::tui::capture::run_capture_tui;
use crate::tui::scan::{FormAction, ScanForm, ScanRun};
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
    ScanForm,
    Scanning,
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
    /// Channel-list filter typed after `/`.
    query: String,
    typing: bool,
    scan_form: Option<ScanForm>,
    scan_run: Option<ScanRun>,
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
        query: String::new(),
        typing: false,
        scan_form: None,
        scan_run: None,
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
        if let Some(run) = state.scan_run.as_mut() {
            if let Some((channels, locked, total)) = run.poll() {
                finish_scan(state, channels, locked, total);
            }
        }
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
            Screen::ScanForm => {
                let action = state.scan_form.as_mut().map(|f| f.on_key(key));
                match action {
                    Some(FormAction::Back) | None => {
                        state.scan_form = None;
                        state.screen = home_screen(state);
                    }
                    Some(FormAction::Start) => start_scan(state),
                    Some(FormAction::None) => {}
                }
            }
            Screen::Scanning => {
                if let Some(run) = state.scan_run.as_mut() {
                    match key.code {
                        KeyCode::Esc | KeyCode::Char('q') => run.confirm_stop = true,
                        KeyCode::Char('y') | KeyCode::Char('Y') if run.confirm_stop => {
                            run.cancel();
                            run.confirm_stop = false;
                        }
                        KeyCode::Char('n') | KeyCode::Char('N') => run.confirm_stop = false,
                        _ => {}
                    }
                }
            }
            Screen::ChannelList if state.typing => match key.code {
                KeyCode::Esc => {
                    state.typing = false;
                    state.query.clear();
                    state.sel_channel = 0;
                    refresh_channel_list(state);
                }
                KeyCode::Enter => state.typing = false,
                KeyCode::Backspace => {
                    state.query.pop();
                    state.sel_channel = 0;
                    refresh_channel_list(state);
                }
                KeyCode::Char(c) => {
                    state.query.push(c);
                    state.sel_channel = 0;
                    refresh_channel_list(state);
                }
                _ => {}
            },
            _ if matches!(key.code, KeyCode::Char('s') | KeyCode::Char('S')) => {
                state.scan_form = Some(ScanForm::new(&state.discovery));
                state.screen = Screen::ScanForm;
            }
            Screen::ChannelList if key.code == KeyCode::Char('/') => {
                state.typing = true;
            }
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
                KeyCode::Esc if !state.query.is_empty() => {
                    state.query.clear();
                    refresh_channel_list(state);
                }
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
                        let watch_result = watch::watch_channel(
                            &paths,
                            &ch,
                            &discovery,
                            &watch::WatchOpts::default(),
                        );
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
                        state.status = match watch_result {
                            Ok(end) if end.user_closed => {
                                "Video closed — pick another channel (↑↓ Enter)".into()
                            }
                            Ok(watch::WatchEnd {
                                tsp_error: Some(err),
                                ..
                            }) => format!("Watch failed: {err}"),
                            Ok(_) => format!("Finished: {}", ch.name),
                            Err(e) => format!("Watch failed: {e:#}"),
                        };
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
    state.channel_ids = filtered_channels(&state.db, state.active_filter, &state.query)
        .into_iter()
        .map(|c| c.id)
        .collect();
    if state.sel_channel >= state.channel_ids.len() {
        state.sel_channel = state.channel_ids.len().saturating_sub(1);
    }
}

fn filtered_channels<'a>(db: &'a ChannelDb, filter: ListFilter, query: &str) -> Vec<&'a SavedChannel> {
    let list = match filter.delivery() {
        Some(d) => db.for_delivery(&d),
        None => db.sorted_all(),
    };
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return list;
    }
    list.into_iter()
        .filter(|c| {
            c.name.to_lowercase().contains(&q)
                || c.provider.to_lowercase().contains(&q)
                || c.satellite.as_deref().is_some_and(|s| s.to_lowercase().contains(&q))
        })
        .collect()
}

/// Screen to return to when leaving the scan screens.
fn home_screen(state: &WatchState) -> Screen {
    if state.db.channels.is_empty() {
        Screen::NoChannels
    } else {
        Screen::DeliveryPick
    }
}

fn start_scan(state: &mut WatchState) {
    let Some(form) = state.scan_form.as_mut() else {
        return;
    };
    match form.targets(&state.discovery) {
        Ok((targets, opts, label)) => {
            lastlog::event("tui.watch", &format!("scan start {} target(s) {label}", targets.len()));
            state.scan_run = Some(ScanRun::start(state.paths.clone(), targets, opts, label));
            state.scan_form = None;
            state.screen = Screen::Scanning;
        }
        Err(e) => form.message = format!("Cannot start: {e:#}"),
    }
}

fn finish_scan(state: &mut WatchState, channels: Vec<SavedChannel>, locked: usize, total: usize) {
    state.scan_run = None;
    let found = channels.len();
    let base = state.db.channels.iter().map(|c| c.scan_index).max().unwrap_or(0);
    let batch = channels
        .into_iter()
        .enumerate()
        .map(|(i, mut c)| {
            c.scan_index = base + 1 + i as u32;
            c
        })
        .collect();
    let added = state.db.add_batch(batch);
    if added > 0 {
        if let Err(e) = state.db.save() {
            state.status = format!("Saving channels failed: {e}");
        }
    }
    state.status = format!("Scan done: {locked}/{total} locked, {found} channel(s), {added} new");
    lastlog::event("tui.watch", &state.status);
    state.delivery_options = build_delivery_options(&state.db);
    state.sel_delivery = 0;
    state.screen = home_screen(state);
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
        Screen::ScanForm => {
            if let Some(form) = &state.scan_form {
                form.draw(f, area);
            }
        }
        Screen::Scanning => {
            if let Some(run) = &state.scan_run {
                run.draw(f, area);
            }
        }
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
        "s scan for channels · i import from captures · c capture TUI · q quit",
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
            "↑↓ PgUp/PgDn · Enter list · s scan · i re-import · c capture · q quit · {}",
            state.status
        ),
    );
}

fn draw_channel_list(f: &mut Frame, area: Rect, state: &WatchState) {
    let chunks = Layout::vertical([Constraint::Min(4), Constraint::Length(KEYS_FOOTER_HEIGHT)]).split(area);
    let channels = filtered_channels(&state.db, state.active_filter, &state.query);
    let items: Vec<String> = channels.iter().map(|c| c.list_label()).collect();
    let title = if state.typing || !state.query.is_empty() {
        format!(
            "Watch — {} — filter: {}{} ({} match)",
            state.active_filter.label(&state.db),
            state.query,
            if state.typing { "_" } else { "" },
            channels.len()
        )
    } else {
        format!("Watch — {}", state.active_filter.label(&state.db))
    };
    list_menu(f, chunks[0], &title, &items, state.sel_channel);
    paragraph(
        f,
        chunks[1],
        "keys",
        &format!(
            "↑↓ PgUp/PgDn · Enter watch · / filter · Esc back · s scan · c capture · q quit · {}",
            state.status
        ),
    );
}

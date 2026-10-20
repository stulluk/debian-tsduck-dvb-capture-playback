//! dvbhub: hybrid DVB capture/playback orchestrator built on TSDuck.
//!
//! This binary is the single entry point for both the command-line interface
//! and (in a later iteration) a local web UI. It drives the supported DVB
//! devices (DTU-315 modulator, TBS5530 and PCTV 461 tuners) by spawning TSDuck
//! tools as subprocesses.

mod analyze;
mod bench;
mod cable;
mod cable_capture;
mod channel_scan;
mod channels;
mod channels_import;
mod config;
mod devices;
mod fe_stats;
mod monitor;
mod pipeline;
mod progress;
mod refdb;
mod satellites;
mod sat_capture;
mod signal;
mod terr_capture;
mod terrestrial;
mod tools;
mod transponders;
mod tsduck;
mod tui;
mod usb;
mod session;
mod lastlog;
mod loopback_compare;
mod watch;
mod web;

use anyhow::Context;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};
use config::TsduckPaths;
use pipeline::{CaptureParams, PlayParams};
use std::thread::sleep;
use std::time::Duration;

/// Top-level command-line interface.
#[derive(Parser)]
#[command(name = "dvbhub", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

/// Available subcommands.
#[derive(Subcommand)]
enum Commands {
    /// List DVB devices usable via TSDuck (and USB-present-but-unusable ones).
    Devices {
        /// Emit machine-readable JSON instead of a human table.
        #[arg(long)]
        json: bool,
    },
    /// List the fixed reference transponders (Turksat 42E).
    Transponders {
        /// Emit machine-readable JSON instead of a human table.
        #[arg(long)]
        json: bool,
    },
    /// Print the resolved TSDuck installation directory and version.
    Tsduck,
    /// Capture a transport stream from a tuner to a file.
    Capture(CaptureArgs),
    /// Capture every reference transponder sequentially from one tuner.
    CaptureAll(CaptureAllArgs),
    /// Play a transport-stream file out through the modulator (DTU-315).
    #[command(name = "playout", visible_aliases = ["play"])]
    Playout(PlayoutArgs),
    /// Refresh transponder lists in ~/.config/dvbhub/satellites.json from KingOfSat.
    UpdateSatellitesJson(UpdateSatellitesJsonArgs),
    /// Loopback test: DTU-315 plays a file, a tuner re-captures it.
    Loopback(LoopbackArgs),
    /// Compare a source TS with its loopback re-capture (L2a link, L2b structure, L2c ES).
    Compare(CompareArgs),
    /// Analyze a transport-stream file: TS id, bitrate, service list.
    Analyze(AnalyzeArgs),
    /// Scan for channels (satellite TPs, UHF band, cable frequencies) and optionally save them.
    Scan(ScanArgs),
    /// Watch a live FTA service: tuner -> tsp zap -> mpv / vlc.
    Watch(WatchArgs),
    /// Start the local web interface (browser UI on localhost).
    Serve(ServeArgs),
    /// Print shell completion scripts (bash: `dvbhub completions bash > /etc/bash_completion.d/dvbhub`).
    Completions {
        /// Target shell.
        #[arg(value_enum)]
        shell: Shell,
    },
}

/// Arguments for the `scan` subcommand.
#[derive(clap::Args)]
struct ScanArgs {
    /// Satellite name from satellites.json (scans its transponders).
    #[arg(long, conflicts_with_all = ["terrestrial", "cable"])]
    satellite: Option<String>,
    /// Only these TPs of the satellite: frequency or frequency+polarity (e.g. 10719v). Repeatable.
    #[arg(long = "tp")]
    tps: Vec<String>,
    /// Scan terrestrial (DVB-T2, then DVB-T); whole UHF band 21-69 unless --frequency-mhz.
    #[arg(long)]
    terrestrial: bool,
    /// Scan cable at the given --frequency-mhz values.
    #[arg(long)]
    cable: bool,
    /// Frequencies in MHz (terrestrial/cable). Repeatable.
    #[arg(long = "frequency-mhz")]
    frequencies_mhz: Vec<u32>,
    /// Terrestrial channel bandwidth in MHz.
    #[arg(long, default_value_t = 8)]
    bandwidth_mhz: u8,
    /// Cable symbol rate in kSymbols/s.
    #[arg(long, default_value_t = 6875)]
    symbol_rate_ksps: u32,
    /// Cable modulation (tsp value).
    #[arg(long, default_value = "256-QAM")]
    modulation: String,
    /// Tuner device `/dev/dvb/adapterA[:F]` (default: auto by delivery).
    #[arg(long)]
    device_name: Option<String>,
    /// Seconds to listen per frequency after lock.
    #[arg(long, default_value_t = 5)]
    listen: u32,
    /// Seconds to wait for lock per frequency.
    #[arg(long, default_value_t = 5)]
    signal_timeout: u32,
    /// Also keep scrambled services.
    #[arg(long)]
    include_scrambled: bool,
    /// Skip radio services.
    #[arg(long)]
    no_radio: bool,
    /// Add the found channels to channels.json.
    #[arg(long)]
    save: bool,
    /// With --save: first remove saved channels of the same delivery (and satellite).
    #[arg(long)]
    replace: bool,
    /// Print results as JSON.
    #[arg(long)]
    json: bool,
}

/// Arguments for the `serve` subcommand.
#[derive(clap::Args)]
struct ServeArgs {
    /// Address to bind the web server to (localhost only by default).
    #[arg(long, default_value = "127.0.0.1")]
    bind: String,
    /// TCP port for the web interface.
    #[arg(long, default_value_t = 8080)]
    port: u16,
}

/// Arguments for the `capture` subcommand.
#[derive(clap::Args)]
struct CaptureArgs {
    /// Reference transponder id (1-4); overrides explicit tuning options.
    #[arg(long)]
    transponder: Option<u32>,
    /// DVB device name `/dev/dvb/adapterA[:F]` (selects the frontend).
    #[arg(long)]
    device_name: Option<String>,
    /// Adapter number (alternative to --device-name).
    #[arg(long)]
    adapter: Option<u32>,
    /// Delivery system (e.g. DVB-S2), required without --transponder.
    #[arg(long)]
    delivery_system: Option<String>,
    /// Carrier/downlink frequency in MHz, required without --transponder.
    #[arg(long)]
    frequency_mhz: Option<u32>,
    /// Polarity: vertical / horizontal (satellite).
    #[arg(long)]
    polarity: Option<String>,
    /// Symbol rate in kSymbols/s.
    #[arg(long)]
    symbol_rate_ksps: Option<u32>,
    /// Modulation (tsp value), e.g. 8-PSK / QPSK.
    #[arg(long)]
    modulation: Option<String>,
    /// Inner FEC, e.g. 3/4.
    #[arg(long)]
    fec: Option<String>,
    /// LNB spec; "0" bypasses LO subtraction (direct L-band injection).
    #[arg(long)]
    lnb: Option<String>,
    /// DiSEqC satellite/dish port (0-63). Default 0 = port A.
    #[arg(long)]
    satellite_number: Option<u32>,
    /// Signal-lock timeout in seconds.
    #[arg(long, default_value_t = 10)]
    signal_timeout: u32,
    /// Capture duration in seconds; 0 runs until interrupted.
    #[arg(long, default_value_t = 10)]
    seconds: u32,
    /// Output transport-stream file path (single-TP plain capture).
    #[arg(long)]
    output: Option<String>,
    /// Interactive capture wizard (ratatui).
    #[arg(long, conflicts_with = "tui_auto")]
    tui: bool,
    /// Non-interactive TUI test path: first tuner, Turksat TP#11054, 5 s.
    #[arg(long, conflicts_with = "tui")]
    tui_auto: bool,
    /// Satellite name from satellites.json (with --full-satellite).
    #[arg(long)]
    satellite: Option<String>,
    /// Capture every transponder of --satellite into per-TP folders.
    #[arg(long)]
    full_satellite: bool,
    /// Output root for --full-satellite (default: capture_dir/<satellite_slug>).
    #[arg(long)]
    output_dir: Option<String>,
    /// Limit transponder count (full-satellite; useful for testing).
    #[arg(long)]
    max_tps: Option<usize>,
    /// With `--full-satellite`: skip TPs that already have a valid capture file.
    #[arg(long)]
    resume: bool,
    /// With `--satellite`: capture one catalogue TP by frequency (MHz, per-TP folders).
    #[arg(long, requires = "satellite")]
    tp_frequency_mhz: Option<u32>,
    /// Disambiguate `--tp-frequency-mhz` (e.g. vertical, horizontal).
    #[arg(long)]
    tp_polarity: Option<String>,
    /// Disambiguate `--tp-frequency-mhz` (symbol rate in kSymbols/s).
    #[arg(long)]
    tp_symbol_rate_ksps: Option<u32>,
    /// After capturing, analyze the file and print its service list.
    #[arg(long)]
    analyze: bool,
    /// Print the tsp command without running it.
    #[arg(long)]
    dry_run: bool,
}

/// Arguments for the `capture-all` subcommand.
#[derive(clap::Args)]
struct CaptureAllArgs {
    /// DVB device name `/dev/dvb/adapterA[:F]` (the tuner to capture from).
    #[arg(long)]
    device_name: Option<String>,
    /// Adapter number (alternative to --device-name).
    #[arg(long)]
    adapter: Option<u32>,
    /// Output directory for the per-transponder capture files.
    #[arg(long, default_value = ".")]
    output_dir: String,
    /// Capture duration per transponder in seconds.
    #[arg(long, default_value_t = 10)]
    seconds: u32,
    /// Signal-lock timeout per transponder in seconds.
    #[arg(long, default_value_t = 15)]
    signal_timeout: u32,
    /// DiSEqC satellite/dish port (0-63). Default 0 = port A.
    #[arg(long)]
    satellite_number: Option<u32>,
    /// Continue with the next transponder even if one fails to lock.
    #[arg(long, default_value_t = true)]
    keep_going: bool,
}

/// Arguments for the `watch` subcommand.
#[derive(clap::Args)]
struct WatchArgs {
    /// Reference transponder id (1-4); overrides explicit tuning options.
    #[arg(long)]
    transponder: Option<u32>,
    /// DVB device name `/dev/dvb/adapterA[:F]` (selects the frontend).
    #[arg(long)]
    device_name: Option<String>,
    /// Adapter number (alternative to --device-name).
    #[arg(long)]
    adapter: Option<u32>,
    /// Service id (program number) to play.
    #[arg(long)]
    service: Option<u64>,
    /// Delivery system (e.g. DVB-S2), required without --transponder.
    #[arg(long)]
    delivery_system: Option<String>,
    /// Carrier/downlink frequency in MHz, required without --transponder.
    #[arg(long)]
    frequency_mhz: Option<u32>,
    /// Polarity: vertical / horizontal (satellite).
    #[arg(long)]
    polarity: Option<String>,
    /// Symbol rate in kSymbols/s.
    #[arg(long)]
    symbol_rate_ksps: Option<u32>,
    /// Modulation (tsp value), e.g. 8-PSK / QPSK.
    #[arg(long)]
    modulation: Option<String>,
    /// Inner FEC, e.g. 3/4.
    #[arg(long)]
    fec: Option<String>,
    /// LNB spec; "0" bypasses LO subtraction (direct L-band injection).
    #[arg(long)]
    lnb: Option<String>,
    /// DiSEqC satellite/dish port (0-63). Default 0 = port A.
    #[arg(long)]
    satellite_number: Option<u32>,
    /// Signal-lock timeout in seconds.
    #[arg(long, default_value_t = watch::WATCH_SIGNAL_TIMEOUT_S)]
    signal_timeout: u32,
    /// Player command reading MPEG-TS from stdin (default: mpv, else vlc).
    #[arg(long)]
    player: Option<String>,
    /// Video only: do not open an audio output (works around a broken audio stack).
    #[arg(long)]
    no_audio: bool,
    /// Print the assembled tsp command without running it.
    #[arg(long)]
    dry_run: bool,
    /// Interactive channel list and live playback (ratatui).
    #[arg(long)]
    tui: bool,
    /// Channel name from ~/.config/dvbhub/channels.json (non-TUI playback).
    #[arg(long)]
    channel: Option<String>,
    /// Headless check: decode video with ffmpeg for --duration seconds and
    /// fail unless frames arrive in real time.
    #[arg(long)]
    test: bool,
    /// Duration of --test in seconds.
    #[arg(long, default_value_t = 30)]
    duration: u32,
}

/// Arguments for the `analyze` subcommand.
#[derive(clap::Args)]
struct AnalyzeArgs {
    /// Transport-stream file to analyze.
    #[arg(long)]
    input: String,
    /// Emit machine-readable JSON instead of a human summary.
    #[arg(long)]
    json: bool,
}

/// Arguments for the `update-satellites-json` subcommand.
#[derive(clap::Args)]
struct UpdateSatellitesJsonArgs {
    /// Refresh only this satellite (case-insensitive name match).
    #[arg(long)]
    satellite: Option<String>,
    /// Fetch and report changes without writing satellites.json.
    #[arg(long)]
    dry_run: bool,
}

/// Arguments for the `playout` subcommand.
#[derive(clap::Args)]
struct PlayoutArgs {
    /// Input transport-stream file path (optional with `--tui`; picked in the wizard).
    #[arg(long)]
    input: Option<String>,
    /// Output L-band IF frequency in MHz (950-2150 for DVB-S/S2).
    #[arg(long, default_value_t = 1200)]
    frequency_mhz: u32,
    /// Dektec modulation token, e.g. DVB-S2-8PSK.
    #[arg(long, default_value = "DVB-S2-8PSK")]
    modulation: String,
    /// Symbol rate in kSymbols/s.
    #[arg(long, default_value_t = 30000)]
    symbol_rate_ksps: u32,
    /// Convolutional/code rate, e.g. 3/4.
    #[arg(long, default_value = "3/4")]
    code_rate: String,
    /// Loop the file forever.
    #[arg(long)]
    infinite: bool,
    /// Output level in 0.1 dBm units (e.g. -300 = -30.0 dBm).
    #[arg(long)]
    level: Option<i32>,
    /// Print the tsp command without running it.
    #[arg(long)]
    dry_run: bool,
    /// Interactive playout wizard (ratatui).
    #[arg(long, conflicts_with = "tui_auto")]
    tui: bool,
    /// Start playout immediately with defaults (non-interactive smoke test).
    #[arg(long, conflicts_with = "tui")]
    tui_auto: bool,
}

/// Arguments for the `loopback` subcommand.
#[derive(clap::Args)]
struct LoopbackArgs {
    /// Input transport-stream file played out by the DTU-315.
    #[arg(long)]
    input: String,
    /// satellite (default), terrestrial or cable.
    #[arg(long, default_value = "satellite")]
    delivery: String,
    /// Satellite: L-band IF in MHz (the tuner gets it with --lnb 0).
    #[arg(long, default_value_t = 1200)]
    if_mhz: u32,
    /// Terrestrial / cable: RF frequency in MHz.
    #[arg(long, default_value_t = 618)]
    frequency_mhz: u32,
    /// Dektec modulation token: DVB-S2-8PSK, DVB-S2-QPSK, DVB-S-QPSK, DVB-T2, DVB-T, 256-QAM, 64-QAM …
    #[arg(long, default_value = "DVB-S2-8PSK")]
    modulation: String,
    /// Symbol rate in kSymbols/s (satellite, cable).
    #[arg(long, default_value_t = 30000)]
    symbol_rate_ksps: u32,
    /// Code rate (satellite FEC, DVB-T2 PLP code rate). Default: 3/4 satellite, 2/3 terrestrial.
    #[arg(long)]
    code_rate: Option<String>,
    /// Terrestrial bandwidth in MHz.
    #[arg(long, default_value_t = 8)]
    bandwidth_mhz: u8,
    /// Terrestrial guard interval.
    #[arg(long, default_value = "19/256")]
    guard_interval: String,
    /// Terrestrial constellation (DVB-T) / PLP modulation (DVB-T2).
    #[arg(long, default_value = "64-QAM")]
    plp_modulation: String,
    /// Capturing tuner device (default: auto by delivery).
    #[arg(long)]
    tuner_device: Option<String>,
    /// Seconds the modulator warms up before the capture tunes.
    #[arg(long, default_value_t = 4)]
    warmup: u64,
    /// Capture duration in seconds.
    #[arg(long, default_value_t = 15)]
    seconds: u32,
    /// Output transport-stream file path for the re-captured stream.
    #[arg(long)]
    output: String,
    /// Skip the L2a/L2b/L2c comparison.
    #[arg(long)]
    no_compare: bool,
    /// Print both tsp commands without running them.
    #[arg(long)]
    dry_run: bool,
}

/// Arguments for the `compare` subcommand.
#[derive(clap::Args)]
struct CompareArgs {
    /// Source transport stream (what the modulator played).
    #[arg(long)]
    source: String,
    /// Re-captured transport stream.
    #[arg(long)]
    capture: String,
    /// Print the report as JSON.
    #[arg(long)]
    json: bool,
}

/// Program entry point.
fn main() -> anyhow::Result<()> {
    lastlog::init();
    let result = main_inner();
    match &result {
        Ok(()) => lastlog::event("main", "exit ok"),
        Err(e) => lastlog::event("main", &format!("exit error: {e:#}")),
    }
    result
}

fn main_inner() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let sub = std::env::args().nth(1).unwrap_or_default();
    lastlog::event("main", &format!("subcommand={sub}"));
    match cli.command {
        Commands::Devices { json } => cmd_devices(json),
        Commands::Transponders { json } => cmd_transponders(json),
        Commands::Tsduck => cmd_tsduck(),
        Commands::Capture(args) => cmd_capture(args),
        Commands::CaptureAll(args) => cmd_capture_all(args),
        Commands::Playout(args) => cmd_playout(args),
        Commands::UpdateSatellitesJson(args) => cmd_update_satellites_json(args),
        Commands::Loopback(args) => cmd_loopback(args),
        Commands::Analyze(args) => cmd_analyze(args),
        Commands::Compare(args) => cmd_compare(args),
        Commands::Scan(args) => cmd_scan(args),
        Commands::Watch(args) => cmd_watch(args),
        Commands::Serve(args) => web::serve_blocking(&args.bind, args.port),
        Commands::Completions { shell } => cmd_completions(shell),
    }
}

/// Implements the `devices` subcommand.
fn cmd_devices(json: bool) -> anyhow::Result<()> {
    lastlog::event("cmd.devices", &format!("json={json}"));
    let paths = TsduckPaths::resolve()?;
    let discovery = devices::discover(&paths)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&discovery)?);
        return Ok(());
    }

    if discovery.devices.is_empty() {
        println!("No DVB devices usable via TSDuck.");
    } else {
        println!("Usable DVB devices:");
        for dev in &discovery.devices {
            let kind = match dev.kind {
                devices::DeviceKind::Tuner => "tuner",
                devices::DeviceKind::Modulator => "modulator",
            };
            let usb = match (&dev.vid_pid, dev.usb_present) {
                (Some(id), true) => format!(" [usb {id}]"),
                _ => String::new(),
            };
            println!(
                "  - {label} ({kind}, {backend} @ {addr}){usb}",
                label = dev.label,
                backend = dev.backend,
                addr = dev.address,
            );
            println!("      model: {}", dev.model);
            if !dev.delivery_systems.is_empty() {
                println!("      delivery: {}", dev.delivery_systems.join(", "));
            }
        }
    }

    for warning in &discovery.warnings {
        println!("  ! {warning}");
    }
    Ok(())
}

/// Implements the `transponders` subcommand.
fn cmd_transponders(json: bool) -> anyhow::Result<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&transponders::REFERENCE_TRANSPONDERS)?
        );
        return Ok(());
    }

    println!("Reference transponders (Turksat 42E):");
    println!(
        "  {:<3} {:>9} {:<4} {:>7} {:<6} {:<5} {:<8} {:>9} {:<5} {}",
        "#", "Freq MHz", "Pol", "SR kS/s", "Mod", "FEC", "System", "IF MHz", "Tone", "Content"
    );
    for tp in transponders::REFERENCE_TRANSPONDERS {
        println!(
            "  {:<3} {:>9} {:<4} {:>7} {:<6} {:<5} {:<8} {:>9} {:<5} {}",
            tp.id,
            tp.frequency_mhz,
            tp.polarity.short(),
            tp.symbol_rate_ksps,
            tp.modulation,
            tp.fec,
            tp.delivery_system,
            tp.l_band_if_mhz(),
            if tp.is_high_band() { "on" } else { "off" },
            tp.content,
        );
    }
    Ok(())
}

/// Implements the `tsduck` subcommand: show installation directory and version.
fn cmd_tsduck() -> anyhow::Result<()> {
    let paths = TsduckPaths::resolve()?;
    println!("TSDuck directory: {}", paths.dir.display());
    let version = tsduck::run_capture(&paths, "tsp", &["--version"])?;
    print!("{version}");
    Ok(())
}

/// Builds capture parameters from the `capture` CLI arguments.
fn capture_params_from_args(args: &CaptureArgs) -> anyhow::Result<CaptureParams> {
    if let Some(id) = args.transponder {
        let tp = transponders::by_id(id)
            .ok_or_else(|| anyhow::anyhow!("unknown transponder id {id} (valid: 1-4)"))?;
        let mut params = CaptureParams::from_transponder(
            tp,
            args.device_name.clone(),
            args.adapter,
            args.seconds,
            args.output.clone().unwrap_or_default(),
        );
        params.signal_timeout_s = args.signal_timeout;
        if let Some(lnb) = &args.lnb {
            params.lnb = Some(lnb.clone());
        }
        if let Some(sn) = args.satellite_number {
            params.satellite_number = Some(sn);
        }
        return Ok(params);
    }

    let frequency_mhz = args.frequency_mhz.ok_or_else(|| {
        anyhow::anyhow!("provide --transponder or --frequency-mhz (+ tuning options)")
    })?;
    let delivery_system = args
        .delivery_system
        .clone()
        .ok_or_else(|| anyhow::anyhow!("--delivery-system is required without --transponder"))?;
    Ok(CaptureParams {
        device_name: args.device_name.clone(),
        adapter: args.adapter,
        delivery_system,
        frequency_hz: frequency_mhz as u64 * 1_000_000,
        polarity: args.polarity.clone(),
        symbol_rate: args.symbol_rate_ksps.map(|s| s as u64 * 1_000),
        modulation: args.modulation.clone(),
        fec: args.fec.clone(),
        lnb: args.lnb.clone(),
        satellite_number: args.satellite_number,
        bandwidth_hz: None,
        guard_interval: None,
        plp: None,
        dvb_status_interval_s: None,
        signal_timeout_s: args.signal_timeout,
        seconds: args.seconds,
        output: args.output.clone().unwrap_or_default(),
        live_analyze: false,
    })
}

/// Implements the `capture` subcommand.
fn cmd_capture(args: CaptureArgs) -> anyhow::Result<()> {
    lastlog::event(
        "cmd.capture",
        &format!("tui={} tui_auto={} seconds={}", args.tui, args.tui_auto, args.seconds),
    );
    let paths = TsduckPaths::resolve()?;

    if args.tui || args.tui_auto {
        return tui::run_capture_tui(paths, args.tui_auto);
    }

    if args.full_satellite {
        return cmd_capture_full_satellite(&paths, &args);
    }

    if let Some(mhz) = args.tp_frequency_mhz {
        return cmd_capture_catalogue_tp(&paths, &args, mhz);
    }

    let output = args
        .output
        .clone()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("--output is required for single-TP capture (or use --full-satellite / --tui)"))?;

    let mut params = capture_params_from_args(&args)?;
    params.output = output;
    params.live_analyze = true;
    cmd_capture_single(&paths, &args, params)
}

fn cmd_capture_full_satellite(paths: &TsduckPaths, args: &CaptureArgs) -> anyhow::Result<()> {
    let sat_name = args
        .satellite
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("--full-satellite requires --satellite NAME"))?;
    let device = args
        .device_name
        .clone()
        .or_else(|| args.adapter.map(|a| format!("/dev/dvb/adapter{a}")))
        .ok_or_else(|| anyhow::anyhow!("--device-name or --adapter is required"))?;

    let db = satellites::SatelliteDb::load();
    let sat = db
        .satellites
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case(sat_name))
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("satellite not in catalogue: {sat_name}"))?;

    if sat.transponders.is_empty() {
        anyhow::bail!("satellite has no transponders — run: dvbhub update-satellites-json");
    }

    let cfg = config::AppConfig::load();
    let slug = sat_capture::satellite_slug(&sat.name);
    let output_root = args
        .output_dir
        .as_ref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| cfg.capture_dir.join(&slug));

    eprintln!(
        "[dvbhub] Full satellite capture: {} → {} ({} s per TP)",
        sat.name,
        output_root.display(),
        args.seconds
    );

    if args.dry_run {
        let limit = args.max_tps.unwrap_or(usize::MAX);
        for tp in sat.transponders.iter().take(limit) {
            println!(
                "{}",
                sat_capture::dry_run_command(
                    &sat,
                    &device,
                    tp,
                    args.seconds,
                    args.signal_timeout,
                    &output_root,
                )
            );
        }
        return Ok(());
    }

    let transponders = sat.transponders.clone();
    let report = sat_capture::run_sat_capture(
        paths,
        sat_capture::SatCaptureOptions {
            satellite: sat,
            device_name: device,
            seconds: args.seconds,
            signal_timeout_s: args.signal_timeout,
            transponders,
            output_root,
            quiet: false,
            max_tps: args.max_tps,
            skip_existing: args.resume,
        },
    )?;

    let locked = report.outcomes.iter().filter(|o| o.run.ever_locked).count();
    eprintln!(
        "[dvbhub] Finished: {}/{} transponders locked, {} skipped or failed",
        locked,
        report.outcomes.len(),
        report.skipped
    );
    Ok(())
}

fn select_catalogue_tp<'a>(
    sat: &'a satellites::Satellite,
    mhz: u32,
    polarity: Option<&str>,
    symbol_rate_ksps: Option<u32>,
) -> anyhow::Result<&'a satellites::SatTransponder> {
    let mut matches: Vec<&satellites::SatTransponder> = sat
        .transponders
        .iter()
        .filter(|t| t.frequency_mhz == mhz)
        .collect();
    if matches.is_empty() {
        anyhow::bail!("no transponder at {mhz} MHz on {}", sat.name);
    }
    if let Some(pol) = polarity {
        let pol = pol.trim();
        matches.retain(|t| {
            t.polarity.eq_ignore_ascii_case(pol)
                || (pol.len() == 1
                    && t.polarity
                        .chars()
                        .next()
                        .is_some_and(|c| c.eq_ignore_ascii_case(&pol.chars().next().unwrap())))
        });
    }
    if let Some(sr) = symbol_rate_ksps {
        matches.retain(|t| t.symbol_rate_ksps == sr);
    }
    match matches.len() {
        0 => anyhow::bail!(
            "no transponder at {mhz} MHz matching polarity={polarity:?} sr={symbol_rate_ksps:?} on {}",
            sat.name
        ),
        1 => Ok(matches[0]),
        n => anyhow::bail!(
            "{n} transponders at {mhz} MHz on {} — use --tp-polarity and/or --tp-symbol-rate-ksps",
            sat.name
        ),
    }
}

fn cmd_capture_catalogue_tp(
    paths: &TsduckPaths,
    args: &CaptureArgs,
    mhz: u32,
) -> anyhow::Result<()> {
    let sat_name = args
        .satellite
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("--tp-frequency-mhz requires --satellite NAME"))?;
    let device = args
        .device_name
        .clone()
        .or_else(|| args.adapter.map(|a| format!("/dev/dvb/adapter{a}")))
        .ok_or_else(|| anyhow::anyhow!("--device-name or --adapter is required"))?;

    let db = satellites::SatelliteDb::load();
    let sat = db
        .satellites
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case(sat_name))
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("satellite not in catalogue: {sat_name}"))?;

    let tp = select_catalogue_tp(&sat, mhz, args.tp_polarity.as_deref(), args.tp_symbol_rate_ksps)?;

    let cfg = config::AppConfig::load();
    let slug = sat_capture::satellite_slug(&sat.name);
    let output_root = args
        .output_dir
        .as_ref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| cfg.capture_dir.join(&slug));

    eprintln!(
        "[dvbhub] Catalogue TP: {} {} MHz {} {} → {} ({} s)",
        sat.name,
        tp.frequency_mhz,
        tp.polarity,
        tp.symbol_rate_ksps,
        output_root.display(),
        args.seconds
    );

    if args.dry_run {
        println!(
            "{}",
            sat_capture::dry_run_command(
                &sat,
                &device,
                tp,
                args.seconds,
                args.signal_timeout,
                &output_root,
            )
        );
        return Ok(());
    }

    let outcome = sat_capture::capture_transponder(
        paths,
        &sat,
        &device,
        tp,
        args.seconds,
        args.signal_timeout,
        &output_root,
        false,
        None,
        None,
    )?;

    eprintln!(
        "[dvbhub] Done: locked={} success={} file={} services={}",
        outcome.run.ever_locked,
        outcome.run.success,
        outcome.ts_path.display(),
        outcome.services_path.display()
    );
    if !outcome.run.success {
        anyhow::bail!("capture failed");
    }
    Ok(())
}

fn cmd_capture_single(
    paths: &TsduckPaths,
    args: &CaptureArgs,
    params: CaptureParams,
) -> anyhow::Result<()> {
    let tsp_args = params.to_tsp_args();

    if args.dry_run {
        println!("{}", tsduck::display_command("tsp", &tsp_args));
        return Ok(());
    }

    let transponder_hint = args.transponder.and_then(|id| {
        transponders::by_id(id).map(|tp| {
            format!(
                "TP#{} — {} MHz {} ({})",
                tp.id,
                tp.frequency_mhz,
                tp.polarity.short(),
                tp.content
            )
        })
    });
    let device_label = resolve_device_label(paths, &params);

    progress::print_capture_intro(
        paths,
        &params,
        transponder_hint.as_deref(),
        device_label.as_deref(),
    );

    let run = progress::run_capture_progress(paths, &tsp_args, params.seconds, params.signal_timeout_s)?;
    progress::print_capture_done(&params.output, &run);

    if !run.success {
        anyhow::bail!("capture failed");
    }

    if args.analyze && !run.services_found {
        note_analyze_start();
        match analyze::analyze_file(paths, &params.output) {
            Ok(summary) => summary.print_human("  "),
            Err(e) => eprintln!("  ! analyze failed: {e}"),
        }
    } else if args.analyze {
        note("Service list above (live analyze); skipping post-capture re-read");
    }
    Ok(())
}

/// Looks up a human-readable label for the capture device, if discovery succeeds.
fn resolve_device_label(paths: &TsduckPaths, params: &CaptureParams) -> Option<String> {
    let addr = params
        .device_name
        .clone()
        .or_else(|| params.adapter.map(|a| format!("/dev/dvb/adapter{a}")))?;
    let discovery = devices::discover(paths).ok()?;
    discovery
        .devices
        .iter()
        .find(|d| d.address == addr)
        .map(|d| format!("{} {}", d.model, d.label))
}

fn note_analyze_start() {
    eprintln!("[dvbhub] Analyzing transport stream…");
}

fn note(msg: &str) {
    eprintln!("[dvbhub] {msg}");
}

/// Implements the `capture-all` subcommand: capture every reference transponder.
fn cmd_capture_all(args: CaptureAllArgs) -> anyhow::Result<()> {
    let paths = TsduckPaths::resolve()?;
    std::fs::create_dir_all(&args.output_dir)
        .with_context(|| format!("cannot create output dir {}", args.output_dir))?;

    let mut failures = 0u32;
    for tp in transponders::REFERENCE_TRANSPONDERS {
        let output = format!(
            "{}/tp{}_{}_{}.ts",
            args.output_dir.trim_end_matches('/'),
            tp.id,
            tp.frequency_mhz,
            tp.polarity.short(),
        );
        println!(
            "\n=== TP#{} {} MHz {} {} {} {} -> {} ===",
            tp.id,
            tp.frequency_mhz,
            tp.polarity.short(),
            tp.modulation,
            tp.fec,
            tp.delivery_system,
            output,
        );

        let mut params = CaptureParams::from_transponder(
            tp,
            args.device_name.clone(),
            args.adapter,
            args.seconds,
            output.clone(),
        );
        params.signal_timeout_s = args.signal_timeout;
        params.satellite_number = args.satellite_number;

        let tsp_args = params.to_tsp_args();
        match tsduck::run_inherit(&paths, "tsp", &tsp_args) {
            Ok(true) => {
                println!("  captured {output}");
                match analyze::analyze_file(&paths, &output) {
                    Ok(summary) => summary.print_human("  "),
                    Err(e) => eprintln!("  ! analyze failed: {e}"),
                }
            }
            Ok(false) | Err(_) => {
                failures += 1;
                eprintln!("  ! TP#{} failed to capture (no lock?)", tp.id);
                if !args.keep_going {
                    anyhow::bail!("capture-all stopped at TP#{} (use --keep-going)", tp.id);
                }
            }
        }
    }

    println!(
        "\ncapture-all done: {} of {} transponders captured.",
        transponders::REFERENCE_TRANSPONDERS.len() as u32 - failures,
        transponders::REFERENCE_TRANSPONDERS.len(),
    );
    if failures > 0 {
        anyhow::bail!("{failures} transponder(s) failed");
    }
    Ok(())
}

/// Builds the tuning `CaptureParams` for a `watch` request (output unused).
fn watch_params(args: &WatchArgs) -> anyhow::Result<CaptureParams> {
    if let Some(id) = args.transponder {
        let tp = transponders::by_id(id)
            .ok_or_else(|| anyhow::anyhow!("unknown transponder id {id} (valid: 1-4)"))?;
        let mut params =
            CaptureParams::from_transponder(tp, args.device_name.clone(), args.adapter, 0, String::new());
        params.signal_timeout_s = args.signal_timeout;
        if let Some(lnb) = &args.lnb {
            params.lnb = Some(lnb.clone());
        }
        if let Some(sn) = args.satellite_number {
            params.satellite_number = Some(sn);
        }
        return Ok(params);
    }
    let frequency_mhz = args.frequency_mhz.ok_or_else(|| {
        anyhow::anyhow!("provide --transponder or --frequency-mhz (+ tuning options)")
    })?;
    let delivery_system = args
        .delivery_system
        .clone()
        .ok_or_else(|| anyhow::anyhow!("--delivery-system is required without --transponder"))?;
    Ok(CaptureParams {
        device_name: args.device_name.clone(),
        adapter: args.adapter,
        delivery_system,
        frequency_hz: frequency_mhz as u64 * 1_000_000,
        polarity: args.polarity.clone(),
        symbol_rate: args.symbol_rate_ksps.map(|s| s as u64 * 1_000),
        modulation: args.modulation.clone(),
        fec: args.fec.clone(),
        lnb: args.lnb.clone(),
        satellite_number: args.satellite_number,
        bandwidth_hz: None,
        guard_interval: None,
        plp: None,
        dvb_status_interval_s: None,
        signal_timeout_s: args.signal_timeout,
        seconds: 0,
        output: String::new(),
        live_analyze: false,
    })
}

/// Implements the `watch` subcommand: live FTA playback to a media player.
///
/// GStreamer watch: `tsp -I dvb … -P zap <sid> -O file → appsrc ! tsdemux ! …`.
fn cmd_watch(args: WatchArgs) -> anyhow::Result<()> {
    lastlog::event(
        "cmd.watch",
        &format!(
            "tui={} test={} channel={:?} service={:?}",
            args.tui, args.test, args.channel, args.service
        ),
    );
    let paths = TsduckPaths::resolve()?;
    if args.tui {
        return tui::run_watch_tui(paths);
    }
    let opts = watch::WatchOpts {
        player_cmd: args.player.clone(),
        no_audio: args.no_audio,
        test_secs: args.test.then_some(args.duration),
        title: None,
    };

    let end = if let Some(name) = args.channel.as_deref() {
        let ch = channels::ChannelDb::load().find_by_name(name)?;
        let opts = watch::WatchOpts {
            title: Some(ch.name.clone()),
            ..opts.clone()
        };
        if args.dry_run {
            let discovery = devices::discover(&paths)?;
            let device = watch::resolve_watch_device(&ch, &discovery)?;
            let tsp_args = watch::watch_tsp_args(&ch, &device, args.signal_timeout)?;
            println!("{}", watch::describe_pipeline(&tsp_args, &watch::player_argv(&opts)?));
            return Ok(());
        }
        let discovery = devices::discover(&paths)?;
        watch::watch_channel(&paths, &ch, &discovery, &opts)?
    } else {
        let service = args
            .service
            .ok_or_else(|| anyhow::anyhow!("--service SID, --channel NAME or --tui is required"))?;
        let params = watch_params(&args)?;
        let mut tsp_args = params.to_tsp_args();
        if let Some(i) = tsp_args.iter().position(|a| a == "-O") {
            tsp_args.truncate(i);
        }
        tsp_args.extend(["-P".into(), "zap".into(), service.to_string()]);
        if args.dry_run {
            println!("{}", watch::describe_pipeline(&tsp_args, &watch::player_argv(&opts)?));
            return Ok(());
        }
        let opts = watch::WatchOpts {
            title: Some(format!("service {service}")),
            ..opts
        };
        watch::run_watch(&paths, &tsp_args, &opts)?
    };

    if let Some(err) = &end.tsp_error {
        anyhow::bail!("watch failed: {err}");
    }
    if let Some(report) = &end.test {
        eprintln!("[dvbhub] watch test: {}", report.summary());
        if !report.passed(args.duration) {
            anyhow::bail!("watch test FAILED — see {}", lastlog::path().display());
        }
        eprintln!("[dvbhub] watch test PASSED");
    }
    Ok(())
}

/// Implements the `scan` subcommand.
fn cmd_scan(args: ScanArgs) -> anyhow::Result<()> {
    use channel_scan::{ScanEvent, ScanOptions};
    use devices::DeliveryCategory;

    let paths = TsduckPaths::resolve()?;
    let discovery = devices::discover(&paths)?;
    let cat = if args.terrestrial {
        DeliveryCategory::Terrestrial
    } else if args.cable {
        DeliveryCategory::Cable
    } else if args.satellite.is_some() {
        DeliveryCategory::Satellite
    } else {
        anyhow::bail!("choose --satellite NAME, --terrestrial or --cable");
    };
    let tuner = discovery.tuner_for(cat, None);
    let device = match (&args.device_name, tuner) {
        (Some(d), _) => d.clone(),
        (None, Some(t)) => t.capture_address(cat),
        (None, None) => anyhow::bail!("no {} tuner found", cat.as_str()),
    };
    let model = tuner.map(|t| t.label.clone());

    let targets = match cat {
        DeliveryCategory::Satellite => {
            let name = args.satellite.as_deref().unwrap_or_default();
            let db = satellites::SatelliteDb::load();
            let sat = db
                .satellites
                .iter()
                .find(|s| s.name.eq_ignore_ascii_case(name))
                .ok_or_else(|| anyhow::anyhow!("satellite not in catalogue: {name}"))?;
            channel_scan::satellite_targets(sat, &device, model.as_deref(), &args.tps)
        }
        DeliveryCategory::Terrestrial => channel_scan::terrestrial_targets(
            &device,
            model.as_deref(),
            &args.frequencies_mhz,
            args.bandwidth_mhz,
        ),
        DeliveryCategory::Cable => {
            anyhow::ensure!(!args.frequencies_mhz.is_empty(), "--cable needs --frequency-mhz");
            channel_scan::cable_targets(
                &device,
                model.as_deref(),
                &args.frequencies_mhz,
                args.symbol_rate_ksps,
                &args.modulation,
            )
        }
    };
    anyhow::ensure!(!targets.is_empty(), "nothing to scan (check --tp filters)");
    let opts = ScanOptions {
        listen_secs: args.listen,
        signal_timeout_s: args.signal_timeout,
        include_scrambled: args.include_scrambled,
        include_radio: !args.no_radio,
    };
    eprintln!(
        "[dvbhub] Scanning {} target(s) on {device}{}",
        targets.len(),
        model.as_deref().map(|m| format!(" ({m})")).unwrap_or_default()
    );
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let results = channel_scan::scan(&paths, &targets, &opts, &cancel, |ev| {
        if let ScanEvent::TargetDone { total, result } = ev {
            let status = if result.locked {
                format!(
                    "LOCK {} SNR {} — {} services, {} kept",
                    result.delivery_system.as_deref().unwrap_or(""),
                    result.snr_db.map(|v| format!("{v:.1} dB")).unwrap_or_else(|| "n/a".into()),
                    result.services.len(),
                    result.wanted_count(&opts)
                )
            } else {
                "no lock".to_string()
            };
            eprintln!("[dvbhub] {}/{} {:<22} {status}", result.index + 1, total, result.label);
        }
    });
    let channels = channel_scan::channels_from_results(&results, &targets, &opts);
    let locked = results.iter().filter(|r| r.locked).count();

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "results": results,
                "channels": channels,
            }))?
        );
    } else {
        for ch in &channels {
            println!("{}", ch.list_label());
        }
    }
    eprintln!(
        "[dvbhub] Scan done: {locked}/{} locked, {} channel(s) found",
        results.len(),
        channels.len()
    );

    if args.save {
        let mut db = channels::ChannelDb::load();
        let delivery = targets[0].delivery.clone();
        if args.replace {
            let sat = targets[0].satellite.clone();
            db.channels.retain(|c| !(c.delivery == delivery && c.satellite == sat));
        }
        let base = db.channels.iter().map(|c| c.scan_index).max().unwrap_or(0);
        let batch: Vec<_> = channels
            .into_iter()
            .enumerate()
            .map(|(i, mut c)| {
                c.scan_index = base + 1 + i as u32;
                c
            })
            .collect();
        let added = db.add_batch(batch);
        db.save()?;
        eprintln!(
            "[dvbhub] Saved {added} new channel(s) to {}",
            config::channels_file_path().display()
        );
    }
    Ok(())
}

/// Writes shell completion scripts to stdout.
fn cmd_completions(shell: Shell) -> anyhow::Result<()> {
    let mut cmd = Cli::command();
    generate(shell, &mut cmd, "dvbhub", &mut std::io::stdout());
    Ok(())
}

/// Implements the `analyze` subcommand.
fn cmd_analyze(args: AnalyzeArgs) -> anyhow::Result<()> {
    let paths = TsduckPaths::resolve()?;
    let summary = analyze::analyze_file(&paths, &args.input)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        summary.print_human("");
    }
    Ok(())
}

/// Implements the `update-satellites-json` subcommand.
fn cmd_update_satellites_json(args: UpdateSatellitesJsonArgs) -> anyhow::Result<()> {
    let path = config::satellites_file_path();
    let mut db = satellites::SatelliteDb::load();
    let filter = args.satellite.as_deref();

    eprintln!(
        "[dvbhub] Refreshing transponders from KingOfSat → {}",
        path.display()
    );
    if let Some(name) = filter {
        eprintln!("[dvbhub] Satellite filter: {name}");
    }
    if args.dry_run {
        eprintln!("[dvbhub] Dry run — satellites.json will not be modified");
    }

    let reports = db.refresh_from_kingofsat(filter)?;
    for r in &reports {
        eprintln!(
            "[dvbhub] {} (pos {} / Kos label {}): {} → {} transponders",
            r.name, r.pos, r.kos_label, r.old_count, r.new_count
        );
    }

    if args.dry_run {
        eprintln!(
            "[dvbhub] Dry run complete — {} satellite(s) would be updated",
            reports.len()
        );
        return Ok(());
    }

    db.save()
        .with_context(|| format!("cannot write {}", path.display()))?;
    eprintln!(
        "[dvbhub] Saved {} — updated {} satellite(s)",
        path.display(),
        reports.len()
    );
    Ok(())
}

/// Implements the `playout` subcommand.
fn cmd_playout(args: PlayoutArgs) -> anyhow::Result<()> {
    lastlog::event(
        "cmd.playout",
        &format!("tui={} input={:?}", args.tui, args.input),
    );
    let paths = TsduckPaths::resolve()?;

    if args.tui || args.tui_auto {
        return tui::run_playout_tui(paths, args.input.clone(), args.tui_auto);
    }

    let discovery = devices::discover(&paths)?;
    let modulator = discovery
        .devices
        .iter()
        .find(|d| d.kind == devices::DeviceKind::Modulator);
    if modulator.is_none() && !args.dry_run {
        anyhow::bail!("no modulator found — connect DTU-315 (or use --dry-run to print the tsp command)");
    }
    if let Some(m) = modulator {
        eprintln!("[dvbhub] Modulator: {} @ {}", m.model, m.address);
    }

    let input = args
        .input
        .clone()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("--input FILE.ts is required (or use --tui)"))?;

    let params = PlayParams {
        input,
        infinite: args.infinite,
        delivery: pipeline::PlayoutDelivery::Satellite,
        frequency_hz: args.frequency_mhz as u64 * 1_000_000,
        modulation: args.modulation.clone(),
        symbol_rate: Some(args.symbol_rate_ksps as u64 * 1_000),
        code_rate: Some(args.code_rate.clone()),
        bandwidth_hz: None,
        guard_interval: None,
        plp_modulation: None,
        j83: None,
        level_dbm_tenths: args.level,
        // A finite, fire-and-forget playout detaches to avoid an end-of-stream
        // hang; an infinite playout stays attached so it keeps transmitting.
        instant_detach: !args.infinite,
    };
    if args.dry_run {
        println!("{}", tsduck::display_command("tsp", &params.to_tsp_args()));
        return Ok(());
    }
    let (tsp_args, note) = pipeline::playout_args(&paths, &params)?;
    eprintln!("[dvbhub] {note}");

    eprintln!(
        "[dvbhub] Playout: {} → {} MHz {} SR {} kSym/s FEC {} {}",
        params.input,
        args.frequency_mhz,
        args.modulation,
        args.symbol_rate_ksps,
        args.code_rate,
        if args.infinite { "(infinite)" } else { "" }
    );
    let ok = tsduck::run_inherit(&paths, "tsp", &tsp_args)?;
    if !ok {
        anyhow::bail!("playout failed (see tsp output above)");
    }
    Ok(())
}

/// Implements the `loopback` subcommand: play out via DTU-315 and re-capture.
fn cmd_loopback(args: LoopbackArgs) -> anyhow::Result<()> {
    use devices::DeliveryCategory;
    let paths = TsduckPaths::resolve()?;
    let cat = match args.delivery.to_ascii_lowercase().as_str() {
        "satellite" | "sat" => DeliveryCategory::Satellite,
        "terrestrial" | "ter" => DeliveryCategory::Terrestrial,
        "cable" | "cab" => DeliveryCategory::Cable,
        other => anyhow::bail!("unknown --delivery {other:?}"),
    };
    let form = refdb::ReferenceCapture {
        name: String::new(),
        file: args.input.clone(),
        delivery: cat.as_str().into(),
        frequency_mhz: if cat == DeliveryCategory::Satellite {
            args.if_mhz
        } else {
            args.frequency_mhz
        },
        modulation: args.modulation.clone(),
        symbol_rate_ksps: args.symbol_rate_ksps,
        code_rate: args.code_rate.clone().unwrap_or_else(|| {
            if cat == DeliveryCategory::Terrestrial { "2/3" } else { "3/4" }.into()
        }),
        bandwidth_mhz: Some(args.bandwidth_mhz),
        guard_interval: Some(args.guard_interval.clone()),
        plp_modulation: Some(args.plp_modulation.clone()),
        j83: None,
        level: None,
        // Stay attached for the whole capture: a detached playout stops once
        // the on-board FIFO drains, breaking the lock.
        infinite: true,
    };
    let play = form.to_play_params(args.input.clone())?;

    let tuner_device = match &args.tuner_device {
        Some(d) => d.clone(),
        None => devices::discover(&paths)?
            .tuner_for(cat, None)
            .map(|d| d.capture_address(cat))
            .ok_or_else(|| anyhow::anyhow!("no {} tuner found; pass --tuner-device", cat.as_str()))?,
    };
    let mut capture = CaptureParams {
        device_name: Some(tuner_device.clone()),
        adapter: None,
        delivery_system: String::new(),
        frequency_hz: play.frequency_hz,
        polarity: None,
        symbol_rate: None,
        modulation: None,
        fec: None,
        lnb: None,
        satellite_number: None,
        bandwidth_hz: None,
        guard_interval: None,
        plp: None,
        dvb_status_interval_s: None,
        signal_timeout_s: 12,
        seconds: args.seconds,
        output: args.output.clone(),
        live_analyze: false,
    };
    match cat {
        DeliveryCategory::Satellite => {
            let (sys, modu) = pipeline::dvb_from_dektec_modulation(&args.modulation)
                .ok_or_else(|| anyhow::anyhow!("unsupported satellite modulation {}", args.modulation))?;
            capture.delivery_system = sys.into();
            capture.modulation = Some(modu.into());
            capture.polarity = Some("vertical".into());
            capture.symbol_rate = play.symbol_rate;
            capture.fec = play.code_rate.clone();
            // Direct L-band injection: no LNB local-oscillator subtraction.
            capture.lnb = Some("0".into());
        }
        DeliveryCategory::Terrestrial => {
            let t2 = args.modulation.eq_ignore_ascii_case("DVB-T2");
            capture.delivery_system = if t2 { "DVB-T2" } else { "DVB-T" }.into();
            capture.bandwidth_hz = play.bandwidth_hz;
            capture.plp = t2.then_some(0);
        }
        DeliveryCategory::Cable => {
            capture.delivery_system = "DVB-C".into();
            capture.symbol_rate = play.symbol_rate;
            capture.modulation = Some(args.modulation.clone());
        }
    }

    let capture_args = capture.to_tsp_args();
    if args.dry_run {
        let play_args = play.to_tsp_args();
        println!("# 1) DTU-315 playout (background, --infinite):");
        println!("{}", tsduck::display_command("tsp", &play_args));
        println!("# 2) tuner re-capture (after {}s warmup):", args.warmup);
        println!("{}", tsduck::display_command("tsp", &capture_args));
        return Ok(());
    }

    let (play_args, note) = pipeline::playout_args(&paths, &play)?;
    println!("Starting DTU-315 playout ({}, attached, infinite): {note}", cat.as_str());
    let mut play_child = tsduck::tool_command(&paths, "tsp").args(&play_args).spawn()?;
    sleep(Duration::from_secs(args.warmup));
    if let Ok(Some(status)) = play_child.try_wait() {
        anyhow::bail!("playout stopped during warm-up ({status}) — see the tsp output above");
    }
    println!("Capturing from {tuner_device} for {}s…", args.seconds);
    let capture_result = tsduck::run_inherit(&paths, "tsp", &capture_args);
    let _ = play_child.kill();
    let _ = play_child.wait();
    anyhow::ensure!(capture_result?, "loopback capture failed (see tsp output above)");
    println!("Loopback capture written to {}", args.output);

    if args.no_compare {
        return Ok(());
    }
    println!("Comparing with the source (L2a / L2b / L2c)…");
    let report = loopback_compare::compare(
        &paths,
        std::path::Path::new(&args.input),
        std::path::Path::new(&args.output),
    )?;
    report.print();
    anyhow::ensure!(report.pass(), "loopback comparison FAILED");
    Ok(())
}

/// Implements the `compare` subcommand.
fn cmd_compare(args: CompareArgs) -> anyhow::Result<()> {
    let paths = TsduckPaths::resolve()?;
    let report = loopback_compare::compare(
        &paths,
        std::path::Path::new(&args.source),
        std::path::Path::new(&args.capture),
    )?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        report.print();
    }
    anyhow::ensure!(report.pass(), "comparison FAILED");
    Ok(())
}

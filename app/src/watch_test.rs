//! Headless watch pipeline test (no TUI): tune, play, collect GStreamer stats.

use crate::channels::ChannelDb;
use crate::config::TsduckPaths;
use crate::devices;
use crate::lastlog;
use crate::watch::{self, WatchOpts};
use crate::watch_gst;

/// Summary parsed from `/tmp/dvbhub_last_log.txt` after a watch test run.
#[derive(Debug, Clone, Default)]
pub struct WatchTestReport {
    pub channel: String,
    pub duration_secs: u32,
    pub mpv_exit_ok: bool,
    pub mpv_exit_code: i32,
    pub max_time_pos: f64,
    pub max_vf_fps: f64,
    pub max_est_frames: u64,
    pub max_ffmpeg_frame: u64,
    pub max_drift_pts_wall: f64,
    pub playback_restart_at: Option<f64>,
    pub stall_detected: bool,
    pub ffmpeg_error_lines: u32,
    pub zap_ready_ms: Option<u64>,
    pub playback_started: bool,
    pub stopped_by_timer: bool,
}

impl WatchTestReport {
    pub fn print_summary(&self) {
        println!("=== watch test: {} ({}s) ===", self.channel, self.duration_secs);
        println!(
            "  pipeline exit: ok={} code={}",
            self.mpv_exit_ok, self.mpv_exit_code
        );
        if let Some(ms) = self.zap_ready_ms {
            println!("  zap ready: +{ms}ms");
        }
        println!(
            "  playback: started={} max_time={:.2}s max_video_bufs={} max_audio_bufs={} ts_bytes={}",
            self.playback_started,
            self.max_time_pos,
            self.max_est_frames,
            self.max_vf_fps as u64,
            self.max_ffmpeg_frame
        );
        println!(
            "  stall={} gst_errors={} stopped_by_timer={}",
            self.stall_detected, self.ffmpeg_error_lines, self.stopped_by_timer
        );
        if self.max_drift_pts_wall.abs() > 0.001 {
            println!(
                "  sync: max |pts-wall| drift={:.2}s (healthy if < 0.5s)",
                self.max_drift_pts_wall
            );
        }
        println!("  log: {}", lastlog::path().display());
    }

    pub fn passed(&self) -> bool {
        let played = self.playback_started
            && (self.max_time_pos >= 2.0 || self.max_est_frames >= 50);
        played && !self.stall_detected && self.ffmpeg_error_lines == 0
    }
}

/// Finds a saved channel by exact or case-insensitive name match.
pub fn find_channel_by_name(name: &str) -> anyhow::Result<crate::channels::SavedChannel> {
    let db = ChannelDb::load();
    let needle = name.trim();
    if let Some(ch) = db.channels.iter().find(|c| c.name == needle) {
        return Ok(ch.clone());
    }
    let lower = needle.to_ascii_lowercase();
    let matches: Vec<_> = db
        .channels
        .iter()
        .filter(|c| c.name.to_ascii_lowercase() == lower || c.name.to_ascii_lowercase().contains(&lower))
        .cloned()
        .collect();
    match matches.len() {
        0 => anyhow::bail!("channel not found in channels.json: {needle:?}"),
        1 => Ok(matches.into_iter().next().unwrap()),
        n => anyhow::bail!("ambiguous channel name {needle:?} ({n} matches)"),
    }
}

/// Runs the full piped watch pipeline headlessly for `duration_secs`, then parses lastlog.
pub fn run_channel_test(
    paths: &TsduckPaths,
    channel_name: &str,
    duration_secs: u32,
    transcode_audio: bool,
    sync_diag: bool,
) -> anyhow::Result<WatchTestReport> {
    let ch = find_channel_by_name(channel_name)?;
    lastlog::event(
        "watch.test",
        &format!(
            "start channel={:?} sid={} duration={duration_secs}s headless",
            ch.name, ch.service_id
        ),
    );

    let discovery = devices::discover(paths)?;
    let opts = WatchOpts {
        headless: true,
        duration_secs: Some(duration_secs),
        transcode_audio,
        sync_diag,
        audio: watch_gst::WatchAudioMode::None,
    };

    let run_result = watch::run_watch_with_opts(paths, &ch, &discovery, opts);

    let mut report = parse_lastlog_report(&ch.name, duration_secs);
    report.mpv_exit_ok = run_result.is_ok();
    if let Err(e) = &run_result {
        lastlog::event("watch.test", &format!("run error: {e:#}"));
        eprintln!("[dvbhub] watch test error: {e:#}");
    }
    report.print_summary();
    run_result?;
    Ok(report)
}

/// Parses GStreamer milestones from the session lastlog written during the test.
pub fn parse_lastlog_report(channel: &str, duration_secs: u32) -> WatchTestReport {
    let path = lastlog::path();
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut report = WatchTestReport {
        channel: channel.to_string(),
        duration_secs,
        ..Default::default()
    };

    for line in text.lines() {
        if line.contains("STALL:") {
            report.stall_detected = true;
        }
        if line.contains("duration limit") && line.contains("stopping pipeline") {
            report.stopped_by_timer = true;
        }
        if line.contains("demux pad video/") || line.contains("video linked") {
            report.playback_started = true;
        }
        if line.contains("[watch]") && line.contains("zap ready:") {
            if let Some(ms) = extract_plus_ms(line) {
                report.zap_ready_ms = Some(ms);
            }
        }
        if line.contains("[gst]") && line.contains("ERROR") {
            report.ffmpeg_error_lines += 1;
        }
        if let Some(rest) = line.split("[gst.sync]").nth(1) {
            if let Some(d) = parse_stat_f64_signed(rest, "drift_pts_wall") {
                report.max_drift_pts_wall = report.max_drift_pts_wall.max(d.abs());
            }
        }
        if let Some(rest) = line.split("[gst.stats]").nth(1) {
            if let Some(tp) = parse_stat_f64(rest, "time") {
                report.max_time_pos = report.max_time_pos.max(tp);
                if tp > 0.1 {
                    report.playback_started = true;
                }
            }
            if let Some(vb) = parse_stat_u64(rest, "video_bufs") {
                report.max_est_frames = report.max_est_frames.max(vb);
                if vb > 10 {
                    report.playback_started = true;
                }
            }
            if let Some(ab) = parse_stat_u64(rest, "audio_bufs") {
                report.max_vf_fps = ab as f64;
            }
            if let Some(ts) = parse_stat_u64(rest, "ts_bytes") {
                report.max_ffmpeg_frame = report.max_ffmpeg_frame.max(ts);
            }
        }
        if line.contains("[gst]") && line.contains("done ok=") {
            report.mpv_exit_ok = line.contains("done ok=true");
        }
    }
    report
}

fn extract_plus_ms(line: &str) -> Option<u64> {
    let after = line.split("[watch] +").nth(1)?;
    let num: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    num.parse().ok()
}

fn parse_stat_f64(text: &str, key: &str) -> Option<f64> {
    let needle = format!("{key}=");
    text.split_whitespace()
        .find(|p| p.starts_with(&needle))?
        .strip_prefix(&needle)?
        .trim_end_matches('s')
        .parse()
        .ok()
}

fn parse_stat_u64(text: &str, key: &str) -> Option<u64> {
    let needle = format!("{key}=");
    text.split_whitespace()
        .find(|p| p.starts_with(&needle))?
        .strip_prefix(&needle)?
        .parse()
        .ok()
}

fn parse_stat_f64_signed(text: &str, key: &str) -> Option<f64> {
    let needle = format!("{key}=");
    text.split_whitespace()
        .find(|p| p.starts_with(&needle))?
        .strip_prefix(&needle)?
        .trim_end_matches('s')
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_stats_line() {
        let line = "18:00:01.000 [mpv.stats] time=3.50s vf_fps=25.0 est_frames=87 drops=0 vo_delayed=0";
        let r = parse_lastlog_report("TRT1 HD", 30);
        // empty log — defaults
        assert_eq!(r.max_time_pos, 0.0);

        let mut fake = String::new();
        fake.push_str(line);
        fake.push('\n');
        std::fs::write("/tmp/dvbhub_watch_test_parse.txt", &fake).unwrap();
        // parse_stat helpers
        assert_eq!(parse_stat_f64(" time=3.50s vf_fps=25.0", "time"), Some(3.5));
    }

    #[test]
    fn find_trt1_hd_in_user_db_if_present() {
        use std::path::Path;
        let path = Path::new("/home/stulluk/.config/dvbhub/channels.json");
        if !path.is_file() {
            return;
        }
        let ch = find_channel_by_name("TRT1 HD").unwrap();
        assert_eq!(ch.service_id, 10601);
    }
}

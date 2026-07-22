//! Piped watch pipeline (tsp stdout → ffmpeg → mpv) with mpv IPC stats.

use crate::lastlog;
use anyhow::Context;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// mpv IPC + file log paths for this watch session (under tmpfs).
pub struct MpvWatchLog {
    pub ipc: PathBuf,
    pub log_file: PathBuf,
}

impl MpvWatchLog {
    pub fn new(session_id: u32) -> Self {
        Self {
            ipc: PathBuf::from(format!("/tmp/dvbhub_mpv_{session_id}.sock")),
            log_file: PathBuf::from(format!("/tmp/dvbhub_mpv_{session_id}.log")),
        }
    }

    pub fn cleanup(&self) {
        let _ = std::fs::remove_file(&self.ipc);
        let _ = std::fs::remove_file(&self.log_file);
    }
}

/// Builds argv for `mpv` (binary name omitted — use with `Command::new("mpv")`).
pub fn mpv_watch_args(log: &MpvWatchLog, headless: bool) -> Vec<String> {
    let mut args = vec![
        format!("--input-ipc-server={}", log.ipc.display()),
        format!("--log-file={}", log.log_file.display()),
        "--msg-level=cplayer=debug,ffmpeg=debug,demuxer=debug,vo=debug".into(),
        "--cache=yes".into(),
        "--demuxer-readahead-secs=0".into(),
        "--term-status-msg=${time-pos}/${duration} vf-fps=${estimated-vf-fps} drop=${decoder-frame-drop-count}"
            .into(),
    ];
    if headless {
        args.extend([
            "--vo=null".into(),
            "--ao=null".into(),
            "--keep-open=always".into(),
            "--really-quiet".into(),
        ]);
    } else {
        args.extend([
            "--force-window=yes".into(),
            "--title=dvbhub".into(),
        ]);
    }
    args.push("-".into());
    args
}

/// Full shell command prefix for dry-run display.
pub fn mpv_shell_cmd(log: &MpvWatchLog, headless: bool) -> String {
    format!("mpv {}", mpv_watch_args(log, headless).join(" "))
}

/// Builds argv for `ffmpeg` remux (binary name omitted — use with `Command::new("ffmpeg")`).
pub fn ffmpeg_watch_args(transcode_audio: bool) -> Vec<String> {
    let mut args = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "warning".into(),
        "-fflags".into(),
        "+genpts+discardcorrupt+nobuffer".into(),
        "-flags".into(),
        "low_delay".into(),
        "-analyzeduration".into(),
        "1000000".into(),
        "-probesize".into(),
        "1000000".into(),
        "-progress".into(),
        "pipe:2".into(),
        "-i".into(),
        "pipe:0".into(),
        "-map".into(),
        "0:v:0".into(),
        "-map".into(),
        "0:a:0?".into(),
        "-c:v".into(),
        "copy".into(),
    ];
    if transcode_audio {
        args.extend([
            "-c:a".into(),
            "aac".into(),
            "-b:a".into(),
            "128k".into(),
            "-ac".into(),
            "2".into(),
        ]);
    } else {
        args.extend(["-c:a".into(), "copy".into()]);
    }
    args.extend([
        "-f".into(),
        "mpegts".into(),
        "-flush_packets".into(),
        "1".into(),
        "pipe:1".into(),
    ]);
    args
}

/// Full shell command for dry-run / fork display.
pub fn ffmpeg_shell_cmd(transcode_audio: bool) -> String {
    format!("ffmpeg {}", ffmpeg_watch_args(transcode_audio).join(" "))
}

/// Spawn-time options for the piped watch pipeline.
pub struct PipeOpts {
    pub headless: bool,
    pub transcode_audio: bool,
}

pub struct PipedWatch {
    tsp: Child,
    ffmpeg: Child,
    mpv: Child,
    mpv_log: MpvWatchLog,
    stop: Arc<AtomicBool>,
    handles: Vec<thread::JoinHandle<()>>,
}

impl PipedWatch {
    /// `tsp_args` must end after `-P zap <sid>` (no `-O fork`).
    pub fn spawn(
        tsp_bin: &Path,
        tsp_args: &[String],
        opts: PipeOpts,
        t0: Instant,
    ) -> anyhow::Result<Self> {
        let session_id = std::process::id();
        let mpv_log = MpvWatchLog::new(session_id);
        mpv_log.cleanup();

        let mut tsp = Command::new(tsp_bin)
            .arg("--verbose")
            .args(tsp_args)
            .arg("-O")
            .arg("file")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawn tsp {}", tsp_bin.display()))?;
        lastlog::event_elapsed("pipe", t0, &format!("tsp pid={}", tsp.id()));

        let tsp_stdout = tsp.stdout.take().context("tsp stdout")?;
        let tsp_stderr = tsp.stderr.take().context("tsp stderr")?;

        let mut ffmpeg = Command::new("ffmpeg")
            .args(ffmpeg_watch_args(opts.transcode_audio))
            .stdin(tsp_stdout)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("spawn ffmpeg")?;
        lastlog::event_elapsed("pipe", t0, &format!("ffmpeg pid={}", ffmpeg.id()));

        let ff_stdout = ffmpeg.stdout.take().context("ffmpeg stdout")?;
        let ff_stderr = ffmpeg.stderr.take().context("ffmpeg stderr")?;

        let mut mpv = Command::new("mpv")
            .args(mpv_watch_args(&mpv_log, opts.headless))
            .stdin(ff_stdout)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("spawn mpv")?;
        lastlog::event_elapsed("pipe", t0, &format!("mpv pid={} ipc={}", mpv.id(), mpv_log.ipc.display()));

        let stop = Arc::new(AtomicBool::new(false));
        let mut handles = Vec::new();

        // tsp stderr
        {
            let stop = Arc::clone(&stop);
            handles.push(thread::spawn(move || {
                let reader = BufReader::new(tsp_stderr);
                for line in reader.lines().map_while(Result::ok) {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    lastlog::event("tsp", &line);
                    log_tsp_milestone(t0, &line);
                }
            }));
        }

        // ffmpeg stderr (includes -progress key=value)
        {
            let stop = Arc::clone(&stop);
            handles.push(thread::spawn(move || {
                let reader = BufReader::new(ff_stderr);
                for line in reader.lines().map_while(Result::ok) {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    if line.contains('=') && !line.starts_with('[') {
                        lastlog::event("ffmpeg", &line);
                        if line.starts_with("frame=") {
                            eprintln!("[dvbhub] ffmpeg {line}");
                        }
                    } else if !line.trim().is_empty() {
                        lastlog::event("ffmpeg", &line);
                        if line.contains("Error") || line.contains("error") {
                            lastlog::event_elapsed("watch", t0, &format!("ffmpeg error: {line}"));
                            eprintln!("[dvbhub] ffmpeg: {line}");
                        }
                    }
                }
            }));
        }

        let mpv_stderr = mpv.stderr.take().context("mpv stderr")?;

        // mpv stderr (status + messages)
        {
            let stop = Arc::clone(&stop);
            handles.push(thread::spawn(move || {
                let reader = BufReader::new(mpv_stderr);
                for line in reader.lines().map_while(Result::ok) {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    lastlog::event("mpv", &line);
                    if should_echo_mpv_line(&line) {
                        eprintln!("[mpv] {line}");
                    }
                }
            }));
        }

        // tail mpv log file
        {
            let path = mpv_log.log_file.clone();
            let stop = Arc::clone(&stop);
            handles.push(thread::spawn(move || tail_mpv_log_file(&path, stop, t0)));
        }

        // mpv IPC stats
        {
            let ipc = mpv_log.ipc.clone();
            let stop = Arc::clone(&stop);
            handles.push(thread::spawn(move || poll_mpv_ipc(&ipc, stop, t0)));
        }

        Ok(Self {
            tsp,
            ffmpeg,
            mpv,
            mpv_log,
            stop,
            handles,
        })
    }

    pub fn wait(mut self, t0: Instant, max_duration: Option<Duration>) -> anyhow::Result<(bool, i32)> {
        let mpv_status = if let Some(limit) = max_duration {
            let deadline = Instant::now() + limit;
            loop {
                match self.mpv.try_wait().context("try_wait mpv")? {
                    Some(status) => break status,
                    None if Instant::now() >= deadline => {
                        lastlog::event_elapsed(
                            "watch.test",
                            t0,
                            &format!("duration limit {:.0}s — stopping mpv", limit.as_secs_f64()),
                        );
                        let _ = self.mpv.kill();
                        break self.mpv.wait().context("wait mpv after kill")?;
                    }
                    None => thread::sleep(Duration::from_millis(200)),
                }
            }
        } else {
            self.mpv.wait().context("wait mpv")?
        };
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.ffmpeg.kill();
        let _ = self.tsp.kill();
        let _ = self.ffmpeg.wait();
        let _ = self.tsp.wait();
        for h in self.handles {
            let _ = h.join();
        }
        let code = mpv_status.code().unwrap_or(-1);
        if t0.elapsed().as_secs() < 3 {
            if let Ok(Some(st)) = self.ffmpeg.try_wait() {
                lastlog::event(
                    "pipe",
                    &format!("ffmpeg early exit code={}", st.code().unwrap_or(-1)),
                );
            }
            if let Ok(Some(st)) = self.tsp.try_wait() {
                lastlog::event(
                    "pipe",
                    &format!("tsp early exit code={}", st.code().unwrap_or(-1)),
                );
            }
        }
        lastlog::event_elapsed("pipe", t0, &format!("mpv exit code={code}"));
        if !mpv_status.success() {
            let _ = std::fs::copy(&self.mpv_log.log_file, lastlog::path().with_extension("mpv.log"));
        }
        self.mpv_log.cleanup();
        Ok((mpv_status.success(), code))
    }
}

fn mpv_log_line_interesting(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    l.contains("demux")
        || l.contains("ffmpeg")
        || l.contains("vo")
        || l.contains("vd")
        || l.contains("ao")
        || l.contains("ad")
        || l.contains("error")
        || l.contains("warn")
        || l.contains("opening")
        || l.contains("video")
        || l.contains("audio")
        || l.contains("cache")
        || l.contains("eof")
        || l.contains("stall")
}

fn should_echo_mpv_line(line: &str) -> bool {
    line.contains("AV:")
        || line.contains("Reading from stdin")
        || line.contains("● Video")
        || line.contains("● Audio")
        || line.contains("VO:")
        || line.contains("AO:")
        || line.contains("vd:")
        || line.contains("demux")
        || line.contains("cplayer:")
        || line.contains("Exiting")
        || line.contains("error")
        || line.contains("Error")
}

fn log_tsp_milestone(t0: Instant, line: &str) {
    let tag = if line.contains("using /dev/dvb") {
        Some("tuner open")
    } else if line.contains("locked: yes") {
        Some("RF locked")
    } else if line.contains("initial input bitrate") {
        Some("TS flowing")
    } else if line.contains("zap: found service") {
        Some("zap ready")
    } else {
        None
    };
    if let Some(tag) = tag {
        lastlog::event_elapsed("watch", t0, &format!("{tag}: {}", line.trim()));
        eprintln!("[dvbhub] +{:.1}s {tag}", t0.elapsed().as_secs_f32());
    }
}

fn tail_mpv_log_file(path: &Path, stop: Arc<AtomicBool>, _t0: Instant) {
    let mut offset = 0u64;
    while !stop.load(Ordering::Relaxed) {
        if let Ok(meta) = std::fs::metadata(path) {
            let len = meta.len();
            if len > offset {
                if let Ok(data) = std::fs::read(path) {
                    if let Ok(text) = std::str::from_utf8(&data[offset as usize..]) {
                        for line in text.lines() {
                            if !line.trim().is_empty() && mpv_log_line_interesting(line) {
                                lastlog::event("mpv.file", line);
                            }
                        }
                    }
                    offset = len;
                }
            }
        }
        thread::sleep(Duration::from_millis(500));
    }
}

fn poll_mpv_ipc(ipc_path: &Path, stop: Arc<AtomicBool>, t0: Instant) {
    let mut stream = None;
    for attempt in 0..80 {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        if let Ok(s) = UnixStream::connect(ipc_path) {
            stream = Some(s);
            lastlog::event_elapsed("mpv.ipc", t0, "connected");
            break;
        }
        if attempt == 0 {
            lastlog::event("mpv.ipc", "waiting for socket…");
        }
        thread::sleep(Duration::from_millis(250));
    }
    let Some(mut stream) = stream else {
        lastlog::event("mpv.ipc", "connect failed (no socket)");
        return;
    };

    let mut reader = BufReader::new(
        stream
            .try_clone()
            .expect("ipc socket try_clone"),
    );

    let props = [
        "time-pos",
        "estimated-vf-fps",
        "decoder-frame-drop-count",
        "vo-delayed-frame-count",
        "pause",
        "core-idle",
        "paused-for-cache",
        "width",
        "height",
    ];

    let mut last_time_pos: Option<f64> = None;
    let mut stall_ticks = 0u32;
    let mut est_frames_shown: u64 = 0;

    while !stop.load(Ordering::Relaxed) {
        let mut parts = Vec::new();
        for prop in props {
            if let Some(val) = mpv_ipc_get_on(&mut stream, &mut reader, prop) {
                parts.push(format!("{prop}={val}"));
            }
        }
        if !parts.is_empty() {
            let line = parts.join(" ");
            lastlog::event("mpv.ipc", &line);

            let time_pos = parse_prop_f64(&line, "time-pos").unwrap_or(0.0);
            let vf_fps = parse_prop_f64(&line, "estimated-vf-fps").unwrap_or(0.0);
            let drops = parse_prop_u64(&line, "decoder-frame-drop-count").unwrap_or(0);
            let delayed = parse_prop_u64(&line, "vo-delayed-frame-count").unwrap_or(0);
            if vf_fps > 0.0 && time_pos > 0.0 {
                est_frames_shown = (time_pos * vf_fps).round() as u64;
            }
            let summary = format!(
                "time={time_pos:.2}s vf_fps={vf_fps:.1} est_frames={est_frames_shown} drops={drops} vo_delayed={delayed}"
            );
            lastlog::event("mpv.stats", &summary);
            eprintln!("[dvbhub] mpv {summary}");

            if let Some(tp) = parse_prop_f64(&line, "time-pos") {
                if let Some(prev) = last_time_pos {
                    if (tp - prev).abs() < 0.01 {
                        stall_ticks += 1;
                        if stall_ticks == 3 {
                            lastlog::event_elapsed(
                                "watch",
                                t0,
                                &format!("STALL: time-pos stuck at {tp:.2}"),
                            );
                            eprintln!("[dvbhub] WARN: playback stalled at {tp:.1}s");
                        }
                    } else {
                        stall_ticks = 0;
                    }
                }
                last_time_pos = Some(tp);
            }
        }
        thread::sleep(Duration::from_secs(1));
    }
}

fn mpv_ipc_get_on(
    stream: &mut UnixStream,
    reader: &mut BufReader<UnixStream>,
    property: &str,
) -> Option<String> {
    static REQ_ID: AtomicU64 = AtomicU64::new(1);
    let id = REQ_ID.fetch_add(1, Ordering::Relaxed);
    let cmd = format!(r#"{{"command":["get_property","{property}"],"request_id":{id}}}"#);
    stream.write_all(cmd.as_bytes()).ok()?;
    stream.write_all(b"\n").ok()?;

    for _ in 0..8 {
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
        if v.get("request_id").and_then(|r| r.as_u64()) != Some(id) {
            continue;
        }
        return parse_ipc_value(&v);
    }
    None
}

fn parse_ipc_value(v: &serde_json::Value) -> Option<String> {
    if v.get("error")?.as_str()? != "success" {
        return None;
    }
    match v.get("data")? {
        serde_json::Value::Null => Some("null".into()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

fn parse_ipc_data(line: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(line.trim())
        .ok()
        .as_ref()
        .and_then(parse_ipc_value)
}

fn parse_prop_f64(line: &str, key: &str) -> Option<f64> {
    parse_prop(line, key)?.parse().ok()
}

fn parse_prop_u64(line: &str, key: &str) -> Option<u64> {
    parse_prop(line, key)?.parse().ok()
}

fn parse_prop(line: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=");
    line.split_whitespace()
        .find(|p| p.starts_with(&needle))?
        .strip_prefix(&needle)
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ipc_success() {
        let line = r#"{"data":12.5,"error":"success","request_id":0}"#;
        assert_eq!(parse_ipc_data(line).as_deref(), Some("12.5"));
    }

    #[test]
    fn ffmpeg_args_no_binary_name() {
        let args = ffmpeg_watch_args(false);
        assert_ne!(args.first().map(String::as_str), Some("ffmpeg"));
        assert!(args.contains(&"-progress".into()));
        assert!(args.contains(&"pipe:2".into()));
        assert!(args.contains(&"pipe:1".into()));
    }
}

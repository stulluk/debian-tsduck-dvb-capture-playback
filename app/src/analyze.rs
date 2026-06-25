//! Transport-stream summaries via TSDuck's `analyze` plugin.
//!
//! After a capture (or for a quick signal/content check) we run
//! `tsp -I file <path> -P analyze --json -O drop` and extract the few fields
//! that matter for the CLI/web UI: transport-stream id, bitrate, and the
//! service list (name, provider, scrambling). This keeps the orchestrator's
//! knowledge of stream content explicit and machine-readable.

use crate::config::TsduckPaths;
use crate::tsduck;
use anyhow::Context;
use serde::{Deserialize, Serialize};

/// One service (program) inside a transport stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceInfo {
    /// Service id (program number).
    pub id: u64,
    /// Service name (e.g. "TRT1 HD").
    pub name: String,
    /// Provider name (e.g. "TRT").
    pub provider: String,
    /// True if any component of the service is scrambled.
    pub scrambled: bool,
}

/// Tuning parameters stored alongside a capture's service list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuneRecord {
    pub frequency_mhz: u32,
    pub polarity: String,
    pub symbol_rate_ksps: u32,
    pub delivery_system: String,
    pub modulation: String,
    pub fec: String,
}

/// Per-TP `services.json` payload written next to `capture.ts`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureServicesRecord {
    pub ts_id: Option<u64>,
    pub bitrate: Option<u64>,
    pub services: Vec<ServiceInfo>,
    pub captured_at: String,
    pub tune: TuneRecord,
}

impl CaptureServicesRecord {
    /// Builds a record from a live or post-capture summary plus TP metadata.
    pub fn from_summary(summary: &StreamSummary, tp: &TuneRecord) -> Self {
        Self {
            ts_id: summary.ts_id,
            bitrate: summary.bitrate,
            services: summary.services.clone(),
            captured_at: chrono::Local::now().to_rfc3339(),
            tune: tp.clone(),
        }
    }

    /// Writes pretty JSON to `path`, creating parent directories if needed.
    pub fn write_file(&self, path: &std::path::Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(path, text)?;
        Ok(())
    }

    /// Reads a capture sidecar written by [`Self::write_file`].
    pub fn read_file(path: &std::path::Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        serde_json::from_str(&text)
            .with_context(|| format!("invalid services JSON: {}", path.display()))
    }
}

/// Sidecar path for `capture.ts` → `capture_services.json`.
pub fn services_json_path_for_ts(ts_path: &std::path::Path) -> std::path::PathBuf {
    let stem = ts_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    ts_path.with_file_name(format!("{stem}_services.json"))
}

/// Compact summary of a transport stream's structure and content.
#[derive(Debug, Clone, Serialize)]
pub struct StreamSummary {
    /// Transport-stream id, if a PAT was found.
    pub ts_id: Option<u64>,
    /// Overall TS bitrate in bits/s (188-byte based), if computed.
    pub bitrate: Option<u64>,
    /// Services discovered in the stream.
    pub services: Vec<ServiceInfo>,
}

impl StreamSummary {
    /// True when at least one service has a real name from SDT (not empty / placeholder).
    pub fn has_named_services(&self) -> bool {
        self.services.iter().any(|s| service_name_is_real(&s.name))
    }

    /// Renders a one-line headline plus a service list for human output.
    pub fn print_human(&self, indent: &str) {
        let ts = self
            .ts_id
            .map(|v| format!("0x{v:04X} ({v})"))
            .unwrap_or_else(|| "unknown".to_string());
        let mbps = self
            .bitrate
            .map(|b| format!("{:.2} Mbps", b as f64 / 1_000_000.0))
            .unwrap_or_else(|| "n/a".to_string());
        println!(
            "{indent}TS id {ts}, {} services, {mbps}",
            self.services.len()
        );
        for s in &self.services {
            let lock = if s.scrambled { "scrambled" } else { "clear" };
            println!(
                "{indent}  - {} (id {}, {}, {})",
                display_service_name(&s.name),
                s.id,
                display_service_name(&s.provider),
                lock
            );
        }
    }
}

/// Returns the best summary for persisting metadata: prefer a full file re-read when
/// live analyze only saw PAT without SDT service names.
pub fn best_capture_summary(
    paths: &TsduckPaths,
    ts_path: &std::path::Path,
    live: Option<&StreamSummary>,
    capture_ok: bool,
) -> Option<StreamSummary> {
    if !capture_ok || !ts_path.is_file() {
        return live.cloned();
    }
    let file_summary = analyze_file(paths, &ts_path.to_string_lossy()).ok();
    match (live, file_summary) {
        (Some(live), Some(file)) if live.has_named_services() && !file.has_named_services() => {
            Some(live.clone())
        }
        (_, Some(file)) => Some(file),
        (Some(live), None) => Some(live.clone()),
        (None, None) => None,
    }
}

/// Lines for TUI / CLI service panes (header + one row per service).
pub fn services_display_lines(summary: &StreamSummary) -> Vec<String> {
    let ts = summary
        .ts_id
        .map(|v| format!("0x{v:04X} ({v})"))
        .unwrap_or_else(|| "unknown".to_string());
    let mbps = summary
        .bitrate
        .map(|b| format!(", {:.2} Mbps", b as f64 / 1_000_000.0))
        .unwrap_or_default();
    let mut lines = vec![format!(
        "Services — TS id {ts}{mbps}, {} service(s)",
        summary.services.len()
    )];
    for s in &summary.services {
        let access = if s.scrambled { "scrambled" } else { "clear" };
        lines.push(format!(
            "  {} (id {}, {}, {access})",
            display_service_name(&s.name),
            s.id,
            display_service_name(&s.provider),
        ));
    }
    lines
}

fn service_name_is_real(name: &str) -> bool {
    let n = name.trim();
    !n.is_empty() && !n.eq_ignore_ascii_case("(unknown)")
}

fn display_service_name(name: &str) -> String {
    if service_name_is_real(name) {
        name.to_string()
    } else {
        "(unknown)".to_string()
    }
}

/// Analyzes a transport-stream file and returns its structural summary.
pub fn analyze_file(paths: &TsduckPaths, path: &str) -> anyhow::Result<StreamSummary> {
    let args = [
        "-I", "file", path, "-P", "analyze", "--json", "-O", "drop",
    ];
    let v = tsduck::run_json(paths, "tsp", &args)?;

    let ts_id = v.get("ts").and_then(|t| t.get("id")).and_then(|i| i.as_u64());
    let bitrate = v
        .get("ts")
        .and_then(|t| t.get("bitrate"))
        .and_then(|b| b.as_u64());

    let mut services = Vec::new();
    if let Some(arr) = v.get("services").and_then(|s| s.as_array()) {
        for s in arr {
            services.push(ServiceInfo {
                id: s.get("id").and_then(|x| x.as_u64()).unwrap_or(0),
                name: s
                    .get("name")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                provider: s
                    .get("provider")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                scrambled: s
                    .get("is-scrambled")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false),
            });
        }
    }

    Ok(StreamSummary {
        ts_id,
        bitrate,
        services,
    })
}

/// Parses a complete `tsp -P analyze --json` stdout document.
pub fn summary_from_analyze_json(text: &str) -> Option<StreamSummary> {
    let v = serde_json::from_str::<serde_json::Value>(text).ok()?;
    let ts_id = v.get("ts").and_then(|t| t.get("id")).and_then(|i| i.as_u64());
    let bitrate = v
        .get("ts")
        .and_then(|t| t.get("bitrate"))
        .and_then(|b| b.as_u64());
    let services = services_from_analyze_value(&v);
    if services.is_empty() && ts_id.is_none() {
        return None;
    }
    Some(StreamSummary {
        ts_id,
        bitrate,
        services,
    })
}

/// Extracts the service list from a `tsp -P analyze --json` document.
pub fn services_from_analyze_json(text: &str) -> Vec<ServiceInfo> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    services_from_analyze_value(&v)
}

fn services_from_analyze_value(v: &serde_json::Value) -> Vec<ServiceInfo> {
    let Some(arr) = v.get("services").and_then(|s| s.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|s| {
            Some(ServiceInfo {
                id: s.get("id")?.as_u64()?,
                name: s
                    .get("name")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                provider: s
                    .get("provider")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                scrambled: s
                    .get("is-scrambled")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false),
            })
        })
        .collect()
}

/// Prints a service list to stderr as soon as live analyze discovers it.
pub fn print_live_services(summary: &StreamSummary) {
    let ts = summary
        .ts_id
        .map(|v| format!("0x{v:04X} ({v})"))
        .unwrap_or_else(|| "unknown".to_string());
    let mbps = summary
        .bitrate
        .map(|b| format!(", {:.2} Mbps", b as f64 / 1_000_000.0))
        .unwrap_or_default();
    eprintln!(
        "[dvbhub] Services on stream — TS id {ts}{mbps}, {} service(s):",
        summary.services.len()
    );
    for s in &summary.services {
        let access = if s.scrambled { "scrambled" } else { "clear" };
        eprintln!(
            "[dvbhub]   - {} (id {}, {}, {access})",
            display_service_name(&s.name),
            s.id,
            display_service_name(&s.provider),
        );
    }
}

/// Accumulates stdout lines until a complete top-level JSON object is seen.
pub fn feed_json_chunk(buf: &mut String, depth: &mut i32, chunk: &str) -> Option<String> {
    for ch in chunk.chars() {
        if *depth == 0 && ch != '{' {
            continue;
        }
        buf.push(ch);
        match ch {
            '{' => *depth += 1,
            '}' => {
                *depth -= 1;
                if *depth == 0 {
                    let doc = buf.clone();
                    buf.clear();
                    return Some(doc);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_named_services_ignores_unknown_placeholder() {
        let empty = StreamSummary {
            ts_id: Some(1),
            bitrate: None,
            services: vec![ServiceInfo {
                id: 1,
                name: "(unknown)".into(),
                provider: "".into(),
                scrambled: false,
            }],
        };
        assert!(!empty.has_named_services());

        let named = StreamSummary {
            ts_id: Some(1),
            bitrate: None,
            services: vec![ServiceInfo {
                id: 1,
                name: "TRT1 HD".into(),
                provider: "TRT".into(),
                scrambled: false,
            }],
        };
        assert!(named.has_named_services());
    }
}

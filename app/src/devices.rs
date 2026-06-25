//! Device discovery: merge TSDuck enumeration with USB presence.
//!
//! Two TSDuck tools are authoritative for usable devices:
//!   * `tsdektec --all --json` lists Dektec modulators (DTU-315 playout).
//!   * `tslsdvb` lists Linux DVB tuners (TBS5530, PCTV 461 capture).
//! `lsusb` is used as a secondary signal to warn about devices that are
//! physically connected but not yet usable (missing driver/firmware).

use crate::config::TsduckPaths;
use crate::tsduck;
use crate::usb;
use serde::Serialize;

/// Whether a device produces a stream (tuner) or consumes one (modulator).
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind {
    /// Source device: captures/demodulates an RF input into a transport stream.
    Tuner,
    /// Sink device: modulates a transport stream onto an RF/ASI output.
    Modulator,
}

/// Broad delivery category used to group capture tabs in the web UI.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryCategory {
    Satellite,
    Terrestrial,
    Cable,
}

impl DeliveryCategory {
    /// Lowercase string form used in JSON and the UI.
    pub fn as_str(self) -> &'static str {
        match self {
            DeliveryCategory::Satellite => "satellite",
            DeliveryCategory::Terrestrial => "terrestrial",
            DeliveryCategory::Cable => "cable",
        }
    }
}

/// Classifies a single delivery-system string into a broad category.
fn delivery_category(system: &str) -> Option<DeliveryCategory> {
    let s = system.to_uppercase();
    if s.starts_with("DVB-S") || s.starts_with("ISDB-S") || s == "DSS" || s == "TURBO" {
        Some(DeliveryCategory::Satellite)
    } else if s.starts_with("DVB-C") {
        Some(DeliveryCategory::Cable)
    } else if s.starts_with("DVB-T")
        || s.starts_with("ISDB-T")
        || s.starts_with("ATSC")
        || s == "DTMB"
        || s == "CMMB"
    {
        Some(DeliveryCategory::Terrestrial)
    } else {
        None
    }
}

/// A device usable through TSDuck, enriched with USB presence information.
#[derive(Debug, Clone, Serialize)]
pub struct DvbDevice {
    /// User-facing label (matched to a known device when possible).
    pub label: String,
    /// Model string reported by the underlying TSDuck tool.
    pub model: String,
    /// Source or sink role.
    pub kind: DeviceKind,
    /// TSDuck backend: "dektec" or "linux-dvb".
    pub backend: String,
    /// Addressing handle: Dektec device index, or Linux DVB device path.
    pub address: String,
    /// Supported delivery systems (e.g. "DVB-S2", "DVB-T2").
    pub delivery_systems: Vec<String>,
    /// Broad categories (satellite/terrestrial/cable) the device supports.
    #[serde(default)]
    pub categories: Vec<String>,
    /// True if a matching known device was found on USB.
    pub usb_present: bool,
    /// USB vendor:product id when matched to a known device.
    pub vid_pid: Option<String>,
}

impl DvbDevice {
    /// Recomputes `categories` from the current `delivery_systems`.
    fn refresh_categories(&mut self) {
        let mut cats: Vec<String> = Vec::new();
        for sys in &self.delivery_systems {
            if let Some(c) = delivery_category(sys) {
                let s = c.as_str().to_string();
                if !cats.contains(&s) {
                    cats.push(s);
                }
            }
        }
        self.categories = cats;
    }
}

/// Result of a discovery pass.
#[derive(Debug, Clone, Serialize)]
pub struct Discovery {
    /// Devices usable through TSDuck right now.
    pub devices: Vec<DvbDevice>,
    /// Advisory messages (e.g. known device present on USB but not usable).
    pub warnings: Vec<String>,
}

/// Performs a full discovery pass: Dektec modulators + Linux DVB tuners + USB.
pub fn discover(paths: &TsduckPaths) -> anyhow::Result<Discovery> {
    let mut devices = Vec::new();
    let mut warnings = Vec::new();

    match discover_modulators(paths) {
        Ok(mut mods) => devices.append(&mut mods),
        Err(e) => warnings.push(format!("Dektec enumeration failed: {e}")),
    }
    match discover_tuners(paths) {
        Ok(mut tuners) => devices.append(&mut tuners),
        Err(e) => warnings.push(format!("DVB tuner enumeration failed: {e}")),
    }

    annotate_usb(&mut devices, &mut warnings);
    for dev in &mut devices {
        dev.refresh_categories();
    }
    Ok(Discovery { devices, warnings })
}

impl Discovery {
    /// Returns the address of a satellite (DVB-S/S2) tuner suitable for a
    /// loopback re-capture. Prefers the TBS5530 S/S2 frontend (the device wired
    /// to the DTU-315 output on this bench); otherwise falls back to any tuner
    /// advertising DVB-S2. Returns `None` if no satellite tuner is usable.
    pub fn satellite_tuner_address(&self) -> Option<String> {
        let supports_sat = |d: &&DvbDevice| {
            d.kind == DeviceKind::Tuner
                && d.delivery_systems
                    .iter()
                    .any(|s| s == "DVB-S2" || s == "DVB-S")
        };
        // Prefer the TBS5530 satellite frontend.
        if let Some(d) = self
            .devices
            .iter()
            .filter(supports_sat)
            .find(|d| d.label.contains("TBS5530") || d.model.to_lowercase().contains("tbs 5530"))
        {
            return Some(d.address.clone());
        }
        // Otherwise any satellite-capable tuner.
        self.devices
            .iter()
            .find(supports_sat)
            .map(|d| d.address.clone())
    }
}

/// Enumerates Dektec modulators via `tsdektec --all --json`.
fn discover_modulators(paths: &TsduckPaths) -> anyhow::Result<Vec<DvbDevice>> {
    let json = tsduck::run_json(paths, "tsdektec", &["--all", "--json"])?;
    let mut out = Vec::new();
    let empty = Vec::new();
    let devices = json
        .get("devices")
        .and_then(|d| d.as_array())
        .unwrap_or(&empty);
    for dev in devices {
        let model = dev
            .get("model")
            .and_then(|m| m.as_str())
            .unwrap_or("Dektec device")
            .to_string();
        let index = dev.get("index").and_then(|i| i.as_i64()).unwrap_or(0);
        let delivery_systems = dektec_delivery_systems(dev);
        out.push(DvbDevice {
            label: model.clone(),
            model,
            kind: DeviceKind::Modulator,
            backend: "dektec".to_string(),
            address: index.to_string(),
            delivery_systems,
            categories: Vec::new(),
            usb_present: false,
            vid_pid: None,
        });
    }
    Ok(out)
}

/// Derives the list of delivery systems a Dektec output supports from its flags.
fn dektec_delivery_systems(dev: &serde_json::Value) -> Vec<String> {
    let mut systems = Vec::new();
    let outputs = dev.get("outputs").and_then(|o| o.as_array());
    let Some(outputs) = outputs else {
        return systems;
    };
    // Flags are exposed per output port; union them across all outputs.
    let flag_map = [
        ("dvb-s", "DVB-S"),
        ("dvb-s2", "DVB-S2"),
        ("dvb-t", "DVB-T"),
        ("dvb-t2", "DVB-T2"),
        ("dvb-c2", "DVB-C2"),
        ("atsc", "ATSC"),
        ("isdb-t", "ISDB-T"),
        ("isdb-s", "ISDB-S"),
        ("dtmb", "DTMB"),
        ("cmmb", "CMMB"),
    ];
    for out in outputs {
        for (flag, name) in flag_map {
            if out.get(flag).and_then(|v| v.as_bool()).unwrap_or(false)
                && !systems.iter().any(|s| s == name)
            {
                systems.push(name.to_string());
            }
        }
    }
    systems
}

/// Enumerates Linux DVB tuners by parsing `tslsdvb` text output.
fn discover_tuners(paths: &TsduckPaths) -> anyhow::Result<Vec<DvbDevice>> {
    let text = tsduck::run_capture(paths, "tslsdvb", &[])?;
    let mut out = Vec::new();
    for line in text.lines() {
        if let Some(dev) = parse_tslsdvb_line(line) {
            out.push(dev);
        }
    }
    Ok(out)
}

/// Parses a single `tslsdvb` line of the form:
/// `0: /dev/dvb/adapter0 ("Model string", DVB-T, DVB-T2, ...)`
fn parse_tslsdvb_line(line: &str) -> Option<DvbDevice> {
    let line = line.trim();
    let (_index, rest) = line.split_once(": ")?;
    let (address, paren) = rest.split_once(" (")?;
    let inner = paren.strip_suffix(')')?;

    // The model is the first double-quoted token; delivery systems follow.
    let after_first_quote = inner.strip_prefix('"')?;
    let (model, tail) = after_first_quote.split_once('"')?;
    let delivery_systems: Vec<String> = tail
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();

    Some(DvbDevice {
        label: model.to_string(),
        model: model.to_string(),
        kind: DeviceKind::Tuner,
        backend: "linux-dvb".to_string(),
        address: address.trim().to_string(),
        delivery_systems,
        categories: Vec::new(),
        usb_present: false,
        vid_pid: None,
    })
}

/// Annotates discovered devices with USB presence and adds warnings for known
/// devices that are connected on USB but not currently usable via TSDuck.
fn annotate_usb(devices: &mut [DvbDevice], warnings: &mut Vec<String>) {
    let present = usb::present_known_devices();
    for known in &present {
        let mut any = false;
        for dev in devices.iter_mut().filter(|d| matches_known(d, known.label)) {
            dev.usb_present = true;
            dev.vid_pid = Some(known.vid_pid.to_string());
            dev.label = known.label.to_string();
            any = true;
        }
        if !any {
            warnings.push(format!(
                "{} ({}) is connected on USB but not usable via TSDuck \
                 (check driver/firmware)",
                known.label, known.vid_pid
            ));
        }
    }
}

/// Heuristic match between a discovered device and a known device label.
fn matches_known(dev: &DvbDevice, known_label: &str) -> bool {
    let model = dev.model.to_lowercase();
    match known_label {
        "DTU-315" => dev.backend == "dektec" || model.contains("dtu-315"),
        "TBS5530" => model.contains("tbs 5530") || model.contains("tbs5530"),
        "PCTV 461" => model.contains("m88ds3103") || model.contains("pctv"),
        _ => false,
    }
}

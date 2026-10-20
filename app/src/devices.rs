//! Device discovery: merge TSDuck enumeration with USB presence.
//!
//! Two TSDuck tools are authoritative for usable devices:
//!   * `tsdektec --all --json` lists Dektec modulators (DTU-315 playout).
//!   * `tslsdvb` lists Linux DVB tuners (TBS5530, PCTV 461 capture).
//! `lsusb` is used as a secondary signal to warn about devices that are
//! physically connected but not yet usable (missing driver/firmware).

use crate::config::TsduckPaths;
use crate::lastlog;
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

/// One selectable delivery path in the capture TUI (filtered per tuner).
#[derive(Debug, Clone, Copy)]
pub struct DeliveryChoice {
    pub category: DeliveryCategory,
    pub label: &'static str,
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

    /// True when this frontend advertises the given delivery category.
    pub fn supports_category(&self, cat: DeliveryCategory) -> bool {
        self.categories.iter().any(|c| c == cat.as_str())
    }

    /// Comma-separated delivery systems for UI labels (`DVB-S, DVB-S2`).
    pub fn delivery_systems_label(&self) -> String {
        if self.delivery_systems.is_empty() {
            "(no delivery systems)".into()
        } else {
            self.delivery_systems.join(", ")
        }
    }

    /// Capture TUI device row: `PCTV 461 — /dev/dvb/adapter0 [DVB-S, DVB-S2]`.
    pub fn tuner_menu_label(&self) -> String {
        format!(
            "{} — {} [{}]",
            self.label,
            self.address,
            self.delivery_systems_label()
        )
    }

    /// Linux DVB device path for capture on the given delivery category.
    pub fn capture_address(&self, category: DeliveryCategory) -> String {
        match category {
            DeliveryCategory::Terrestrial | DeliveryCategory::Cable
                if self.backend == "linux-dvb" && !self.address.contains(':') =>
            {
                frontend0_device_name(&self.address)
            }
            _ => self.address.clone(),
        }
    }
}

/// Delivery capture modes offered for one tuner (derived from ENUM_DELSYS / tslsdvb).
pub fn delivery_choices(device: &DvbDevice) -> Vec<DeliveryChoice> {
    let mut out = Vec::new();
    if device.supports_category(DeliveryCategory::Satellite) {
        out.push(DeliveryChoice {
            category: DeliveryCategory::Satellite,
            label: "Satellite (DVB-S / S2)",
        });
    }
    if device.supports_category(DeliveryCategory::Terrestrial) {
        out.push(DeliveryChoice {
            category: DeliveryCategory::Terrestrial,
            label: "Terrestrial (DVB-T / T2)",
        });
    }
    if device.supports_category(DeliveryCategory::Cable) {
        out.push(DeliveryChoice {
            category: DeliveryCategory::Cable,
            label: "Cable (DVB-C)",
        });
    }
    out
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
    lastlog::event(
        "devices",
        &format!(
            "discover ok tuners={} modulators={} warnings={}",
            devices.iter().filter(|d| d.kind == DeviceKind::Tuner).count(),
            devices.iter().filter(|d| d.kind == DeviceKind::Modulator).count(),
            warnings.len()
        ),
    );
    for w in &warnings {
        lastlog::event("devices.warn", w);
    }
    for d in &devices {
        lastlog::event(
            "devices",
            &format!(
                "{} kind={:?} addr={} cats=[{}]",
                d.label,
                d.kind,
                d.address,
                d.categories.join(",")
            ),
        );
    }
    Ok(Discovery { devices, warnings })
}

impl Discovery {
    /// Returns capture tuners that advertise at least one delivery category.
    pub fn capture_tuners(&self) -> Vec<&DvbDevice> {
        self.devices
            .iter()
            .filter(|d| d.kind == DeviceKind::Tuner && !d.categories.is_empty())
            .collect()
    }

    /// True when at least one usable capture tuner was discovered.
    pub fn has_capture_tuners(&self) -> bool {
        !self.capture_tuners().is_empty()
    }

    /// Picks a tuner for `cat`, preferring one whose label/model matches `model`
    /// (stable across re-plugging, unlike `/dev/dvb/adapterN`).
    pub fn tuner_for(&self, cat: DeliveryCategory, model: Option<&str>) -> Option<&DvbDevice> {
        let wanted = model.map(str::to_ascii_lowercase);
        wanted
            .as_deref()
            .and_then(|m| {
                self.devices.iter().find(|d| {
                    d.kind == DeviceKind::Tuner
                        && d.supports_category(cat)
                        && (d.label.to_ascii_lowercase().contains(m)
                            || d.model.to_ascii_lowercase().replace(' ', "").contains(m))
                })
            })
            .or_else(|| self.tuner_for_category(cat))
    }

    /// Picks a tuner for the given delivery category (bench-aware preferences).
    pub fn tuner_for_category(&self, cat: DeliveryCategory) -> Option<&DvbDevice> {
        let supports = |d: &&DvbDevice| {
            d.kind == DeviceKind::Tuner && d.supports_category(cat)
        };
        match cat {
            DeliveryCategory::Satellite => self
                .devices
                .iter()
                .filter(supports)
                .find(|d| d.label.contains("PCTV") || d.label.contains("461"))
                .or_else(|| self.devices.iter().find(supports)),
            DeliveryCategory::Terrestrial => self
                .devices
                .iter()
                .filter(supports)
                .find(|d| d.label.contains("TBS5530") || d.model.to_lowercase().contains("tbs 5530"))
                .or_else(|| self.devices.iter().find(supports)),
            DeliveryCategory::Cable => self
                .devices
                .iter()
                .filter(supports)
                .find(|d| d.label.contains("TBS5530") || d.model.to_lowercase().contains("tbs 5530"))
                .or_else(|| self.devices.iter().find(supports)),
        }
    }

    /// Returns the Linux DVB device name for terrestrial capture on the TBS5530
    /// frontend 0 (`adapterN:0`). Falls back to any terrestrial-capable tuner.
    pub fn terrestrial_tuner_address(&self) -> Option<String> {
        let supports_terr = |d: &&DvbDevice| {
            d.kind == DeviceKind::Tuner
                && d
                    .categories
                    .iter()
                    .any(|c| c == DeliveryCategory::Terrestrial.as_str())
        };
        if let Some(d) = self
            .devices
            .iter()
            .filter(supports_terr)
            .find(|d| d.label.contains("TBS5530") || d.model.to_lowercase().contains("tbs 5530"))
        {
            return Some(frontend0_device_name(&d.address));
        }
        self.devices
            .iter()
            .find(supports_terr)
            .map(|d| frontend0_device_name(&d.address))
    }
}

/// Selects DVB-T/T2 frontend 0 on a multi-frontend adapter.
fn frontend0_device_name(address: &str) -> String {
    if address.contains(':') {
        address.to_string()
    } else {
        format!("{address}:0")
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
        // TSDuck reports cable QAM annexes under combined keys.
        ("qam:qam-a:dvb-c", "DVB-C"),
        ("qam:qam-b", "DVB-C/B"),
        ("qam:qam-c", "DVB-C/C"),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_sat() -> DvbDevice {
        DvbDevice {
            label: "PCTV 461".into(),
            model: "Montage Technology M88DS3103C".into(),
            kind: DeviceKind::Tuner,
            backend: "linux-dvb".into(),
            address: "/dev/dvb/adapter0".into(),
            delivery_systems: vec!["DVB-S".into(), "DVB-S2".into()],
            categories: vec!["satellite".into()],
            usb_present: true,
            vid_pid: None,
        }
    }

    fn sample_terr() -> DvbDevice {
        DvbDevice {
            label: "TBS5530".into(),
            model: "TurboSight TBS 5530 DVB-T/T2".into(),
            kind: DeviceKind::Tuner,
            backend: "linux-dvb".into(),
            address: "/dev/dvb/adapter1".into(),
            delivery_systems: vec![
                "DVB-T".into(),
                "DVB-T2".into(),
                "DVB-C".into(),
                "ATSC".into(),
            ],
            categories: vec!["terrestrial".into(), "cable".into()],
            usb_present: true,
            vid_pid: None,
        }
    }

    #[test]
    fn delivery_choices_sat_only() {
        let choices = delivery_choices(&sample_sat());
        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0].category, DeliveryCategory::Satellite);
    }

    #[test]
    fn delivery_choices_terr_and_cable() {
        let choices = delivery_choices(&sample_terr());
        assert_eq!(choices.len(), 2);
        assert_eq!(choices[0].category, DeliveryCategory::Terrestrial);
        assert_eq!(choices[1].category, DeliveryCategory::Cable);
    }

    #[test]
    fn tuner_menu_label_includes_delsys() {
        let label = sample_sat().tuner_menu_label();
        assert!(label.contains("PCTV 461"));
        assert!(label.contains("DVB-S2"));
    }

    #[test]
    fn terr_capture_address_adds_frontend0() {
        assert_eq!(
            sample_terr().capture_address(DeliveryCategory::Terrestrial),
            "/dev/dvb/adapter1:0"
        );
        assert_eq!(
            sample_sat().capture_address(DeliveryCategory::Satellite),
            "/dev/dvb/adapter0"
        );
    }

    #[test]
    fn dektec_dtu315_flags_include_cable() {
        let dev = serde_json::json!({ "outputs": [{
            "dvb-s": true, "dvb-s2": true, "dvb-t": true, "dvb-t2": false, "atsc": true,
            "qam:qam-a:dvb-c": true, "qam:qam-b": true, "qam:qam-c": true
        }]});
        let sys = dektec_delivery_systems(&dev);
        assert!(sys.contains(&"DVB-C".to_string()), "{sys:?}");
        assert!(sys.contains(&"DVB-S2".to_string()));
        assert!(!sys.contains(&"DVB-T2".to_string()));
    }
}

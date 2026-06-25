//! USB-level detection of the supported DVB devices.
//!
//! Before TSDuck can use a device, the device must be physically connected and
//! its kernel driver bound. Detecting the raw USB presence (via `lsusb`) lets the
//! orchestrator distinguish "device not plugged in" from "plugged in but no
//! driver / not usable", which drives the user guidance flow.

use std::process::Command;

/// A DVB device model supported by this project, keyed by its cold-state USB ID.
#[derive(Debug, Clone, Copy)]
pub struct KnownDevice {
    /// Short, user-facing label.
    pub label: &'static str,
    /// USB vendor:product id in lowercase hex, e.g. "1297:030f".
    pub vid_pid: &'static str,
    /// Human-readable role of the device (shown in UI; not yet read in CLI).
    #[allow(dead_code)]
    pub role: &'static str,
}

/// The three devices this project targets (see docs/hardware_devices.md).
pub const KNOWN_DEVICES: &[KnownDevice] = &[
    KnownDevice {
        label: "DTU-315",
        vid_pid: "1297:030f",
        role: "Dektec DVB modulator (playout)",
    },
    KnownDevice {
        label: "TBS5530",
        vid_pid: "734c:5530",
        role: "Multi-standard DVB tuner (capture)",
    },
    KnownDevice {
        label: "PCTV 461",
        vid_pid: "2013:0462",
        role: "DVB-S/S2 tuner (capture)",
    },
];

/// Returns the set of known-device `vid:pid` values currently present on USB.
///
/// Uses `lsusb`; if `lsusb` is unavailable the result is an empty set rather
/// than an error, since USB presence is advisory (TSDuck enumeration is the
/// authoritative source for usable devices).
pub fn present_known_devices() -> Vec<&'static KnownDevice> {
    let listing = match lsusb_listing() {
        Some(text) => text,
        None => return Vec::new(),
    };
    KNOWN_DEVICES
        .iter()
        .filter(|dev| listing.contains(dev.vid_pid))
        .collect()
}

/// Captures `lsusb` output, or `None` if the command is not runnable.
fn lsusb_listing() -> Option<String> {
    let output = Command::new("lsusb").output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).to_lowercase())
}

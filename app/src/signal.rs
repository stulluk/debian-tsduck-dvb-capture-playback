//! RF / demodulator status parsed from TSDuck `tsp` verbose output.
//!
//! Shared by the web session manager, CLI progress reporting, and scan parsing.

use serde::Serialize;

/// Live tuner status (lock, strength, SNR, BER).
#[derive(Debug, Clone, Default, Serialize)]
pub struct SignalStatus {
    pub locked: bool,
    pub strength_db: Option<f64>,
    pub snr_db: Option<f64>,
    pub ber: Option<String>,
    pub raw: Option<String>,
}

impl SignalStatus {
    /// One-line human summary for CLI progress output.
    pub fn summary(&self) -> String {
        let strength = self
            .strength_db
            .map(|v| format!("{v:.1} dB"))
            .unwrap_or_else(|| "n/a".to_string());
        let snr = self
            .snr_db
            .map(|v| format!("{v:.1} dB"))
            .unwrap_or_else(|| "n/a".to_string());
        let ber = self.ber.as_deref().unwrap_or("n/a");
        if self.locked {
            format!("LOCKED — strength {strength}, SNR {snr}, BER {ber}")
        } else {
            format!("NO LOCK — strength {strength}, SNR {snr}")
        }
    }

    /// Compact metrics block for the capture TUI signal pane.
    pub fn metrics_panel(sig: Option<&SignalStatus>, bitrate: Option<&str>) -> String {
        let quality = sig
            .and_then(|s| s.snr_db)
            .map(|v| format!("{v:.1} dB"))
            .unwrap_or_else(|| "n/a".into());
        let strength = sig
            .and_then(|s| s.strength_db)
            .map(|v| format!("{v:.1} dB"))
            .unwrap_or_else(|| "n/a".into());
        let ber = sig
            .and_then(|s| s.ber.as_deref())
            .unwrap_or("n/a");
        let rate = bitrate.unwrap_or("n/a");
        format!(
            "Quality: {quality}\nStrength: {strength}\nBER: {ber}\nBitrate: {rate}"
        )
    }
}

/// Parses `* dvb: locked: yes, strength: -45.8 dB, SNR: 19.031 dB` lines.
pub fn parse_signal_line(line: &str) -> Option<SignalStatus> {
    if !line.contains("dvb:") || !line.contains("locked:") {
        return None;
    }
    let locked = line.contains("locked: yes");
    let strength_db = extract_float_after(line, "strength:");
    let snr_db = extract_float_after(line, "SNR:");
    let ber = line
        .split("BER:")
        .nth(1)
        .map(|s| s.trim().trim_end_matches(')').to_string());
    Some(SignalStatus {
        locked,
        strength_db,
        snr_db,
        ber,
        raw: Some(line.trim().to_string()),
    })
}

fn extract_float_after(line: &str, key: &str) -> Option<f64> {
    let rest = line.split(key).nth(1)?;
    rest.split_whitespace().next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_locked_line() {
        let sig = parse_signal_line(
            "* dvb: locked: yes, strength: -45.8 dB, SNR: 19.031 dB, BER: 0%",
        )
        .unwrap();
        assert!(sig.locked);
        assert!((sig.strength_db.unwrap() + 45.8).abs() < 0.01);
        assert!((sig.snr_db.unwrap() - 19.031).abs() < 0.001);
    }
}

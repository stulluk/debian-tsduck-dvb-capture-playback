//! NIT-based network scan via TSDuck `tsscan`.
//!
//! Parses the text output of `tsscan --nit-scan` into structured transponder
//! entries for the web UI catalog.

use serde::Serialize;

/// One transponder entry discovered during a NIT scan.
#[derive(Debug, Clone, Serialize)]
pub struct ScannedTransponder {
    /// Downlink frequency in Hz.
    pub frequency_hz: u64,
    pub polarity: String,
    pub delivery_system: Option<String>,
    pub modulation: Option<String>,
    pub locked: bool,
    pub strength_db: Option<f64>,
    pub snr_db: Option<f64>,
    pub ts_id: Option<u64>,
    pub raw_line: String,
}

/// Parses `tsscan` stdout lines into transponder records.
pub fn parse_tsscan_output(text: &str) -> Vec<ScannedTransponder> {
    let mut out = Vec::new();
    let mut current: Option<ScannedTransponder> = None;

    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("* Frequency:") {
            if let Some(tp) = current.take() {
                out.push(tp);
            }
            current = Some(parse_frequency_line(line));
        } else if line.starts_with("Transport stream id:") {
            if let Some(ref mut tp) = current {
                if let Some(id) = parse_ts_id(line) {
                    tp.ts_id = Some(id);
                }
            }
        }
    }
    if let Some(tp) = current {
        out.push(tp);
    }
    out
}

fn parse_frequency_line(line: &str) -> ScannedTransponder {
    // * Frequency: 11,054,000,000 Hz V (DVB-S2, 8-PSK), locked: yes, strength: -79 dB, SNR: 13.4 dB, BER: 0%
    let locked = line.contains("locked: yes");
    let strength_db = extract_float_after(line, "strength:");
    let snr_db = extract_float_after(line, "SNR:");

    let freq_hz = line
        .split("Frequency:")
        .nth(1)
        .and_then(|s| s.split_whitespace().next())
        .and_then(|s| s.replace(',', "").parse::<u64>().ok())
        .unwrap_or(0);

    let polarity = if line.contains(" Hz H") || line.contains(" Hz H,") {
        "horizontal".to_string()
    } else if line.contains(" Hz V") || line.contains(" Hz V,") {
        "vertical".to_string()
    } else {
        "unknown".to_string()
    };

    let delivery_system = line
        .split('(')
        .nth(1)
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string());

    let modulation = line
        .split('(')
        .nth(1)
        .and_then(|s| s.split(',').nth(1))
        .map(|s| s.trim().trim_end_matches(')').to_string());

    ScannedTransponder {
        frequency_hz: freq_hz,
        polarity,
        delivery_system,
        modulation,
        locked,
        strength_db,
        snr_db,
        ts_id: None,
        raw_line: line.to_string(),
    }
}

fn parse_ts_id(line: &str) -> Option<u64> {
    // Transport stream id: 31001, 0x7919
    line.split(':')
        .nth(1)
        .and_then(|s| s.split(',').next())
        .and_then(|s| s.trim().parse().ok())
}

fn extract_float_after(line: &str, key: &str) -> Option<f64> {
    let rest = line.split(key).nth(1)?;
    rest.split_whitespace().next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sample_lines() {
        let text = "* Frequency: 11,054,000,000 Hz V (DVB-S2, 8-PSK), locked: yes, SNR: 13.4 dB\n  Transport stream id: 31001, 0x7919\n";
        let tps = parse_tsscan_output(text);
        assert_eq!(tps.len(), 1);
        assert_eq!(tps[0].frequency_hz, 11_054_000_000);
        assert_eq!(tps[0].ts_id, Some(31001));
        assert!(tps[0].locked);
    }
}

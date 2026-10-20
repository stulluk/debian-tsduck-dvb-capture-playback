//! Builders for TSDuck `tsp` pipelines (capture and playout).
//!
//! These pure functions produce the exact `tsp` argument vectors that the CLI
//! either prints (`--dry-run`) or executes. Keeping them separate makes the
//! command construction explicit and unit-testable, independent of hardware.
//!
//! The flag names and accepted values were verified against the deployed
//! TSDuck build (`tsp -I dvb --help`, `tsp -O dektec --help`).

use crate::analyze::TuneRecord;
use crate::transponders::Transponder;

/// Parameters for a DVB capture (tuner -> transport-stream file).
#[derive(Debug, Clone)]
pub struct CaptureParams {
    /// Full DVB device name `/dev/dvb/adapterA[:F]` (selects the frontend).
    pub device_name: Option<String>,
    /// Adapter number (alternative to `device_name`).
    pub adapter: Option<u32>,
    /// Delivery system, e.g. "DVB-S2".
    pub delivery_system: String,
    /// Carrier frequency in Hz (for satellite this is the downlink frequency,
    /// unless `lnb` bypasses the local-oscillator subtraction).
    pub frequency_hz: u64,
    /// Polarity (satellite only): "vertical" / "horizontal".
    pub polarity: Option<String>,
    /// Symbol rate in symbols/s.
    pub symbol_rate: Option<u64>,
    /// Modulation (`tsp` dvb value), e.g. "8-PSK", "QPSK".
    pub modulation: Option<String>,
    /// Inner FEC, e.g. "3/4".
    pub fec: Option<String>,
    /// LNB spec; "0" bypasses LO subtraction (direct L-band injection).
    pub lnb: Option<String>,
    /// DiSEqC satellite/dish port (0-63); default 0 = port A.
    pub satellite_number: Option<u32>,
    /// Terrestrial channel bandwidth in Hz (`--bandwidth`).
    pub bandwidth_hz: Option<u64>,
    /// Terrestrial guard interval (`--guard-interval`, e.g. `19/256`).
    pub guard_interval: Option<String>,
    /// DVB-T2 physical layer pipe (`--plp`).
    pub plp: Option<u8>,
    /// When set, pass `--json-line` / `--json-interval` for periodic RF status (DVB-T/T2).
    pub dvb_status_interval_s: Option<u32>,
    /// Signal-lock timeout in seconds.
    pub signal_timeout_s: u32,
    /// Capture duration in seconds; 0 means run until stopped.
    pub seconds: u32,
    /// Output transport-stream file path.
    pub output: String,
    /// When true, insert `-P analyze --json` so service names are available
    /// while the capture runs (stdout JSON, same as the web tune monitor).
    pub live_analyze: bool,
}

impl CaptureParams {
    /// Builds the `tsp` argument vector for this capture.
    pub fn to_tsp_args(&self) -> Vec<String> {
        let mut a: Vec<String> = vec!["-I".into(), "dvb".into()];
        if let Some(name) = &self.device_name {
            a.push("--device-name".into());
            a.push(name.clone());
        }
        if let Some(ad) = self.adapter {
            a.push("--adapter".into());
            a.push(ad.to_string());
        }
        a.push("--delivery-system".into());
        a.push(self.delivery_system.clone());
        a.push("--frequency".into());
        a.push(self.frequency_hz.to_string());
        if let Some(p) = &self.polarity {
            a.push("--polarity".into());
            a.push(p.clone());
        }
        if let Some(sr) = self.symbol_rate {
            a.push("--symbol-rate".into());
            a.push(sr.to_string());
        }
        if let Some(m) = &self.modulation {
            a.push("--modulation".into());
            a.push(m.clone());
        }
        // TSDuck documents `--fec-inner` for satellite/cable only; passing it on
        // DVB-T/T2 breaks lock on the TBS5530 (driver ignores HP/LP FEC flags).
        if let Some(f) = &self.fec {
            if !is_terrestrial_delivery(&self.delivery_system) {
                a.push("--fec-inner".into());
                a.push(f.clone());
            }
        }
        if let Some(l) = &self.lnb {
            a.push("--lnb".into());
            a.push(l.clone());
        }
        if let Some(sn) = self.satellite_number {
            a.push("--satellite-number".into());
            a.push(sn.to_string());
        }
        if let Some(bw) = self.bandwidth_hz {
            a.push("--bandwidth".into());
            a.push(bw.to_string());
        }
        if let Some(gi) = &self.guard_interval {
            a.push("--guard-interval".into());
            a.push(gi.clone());
        }
        if let Some(plp) = self.plp {
            a.push("--plp".into());
            a.push(plp.to_string());
        }
        if let Some(sec) = self.dvb_status_interval_s {
            a.push("--json-line".into());
            a.push("--json-interval".into());
            a.push(sec.to_string());
        }
        a.push("--signal-timeout".into());
        a.push(self.signal_timeout_s.to_string());
        if self.live_analyze {
            a.extend([
                "-P".into(),
                "analyze".into(),
                "--json".into(),
                "--interval".into(),
                "3".into(),
            ]);
        }
        if self.seconds > 0 {
            a.push("-P".into());
            a.push("until".into());
            a.push("--seconds".into());
            a.push(self.seconds.to_string());
        }
        a.push("-O".into());
        a.push("file".into());
        a.push(self.output.clone());
        a
    }

    /// Builds capture params for a reference transponder on a given tuner.
    pub fn from_transponder(
        tp: &Transponder,
        device_name: Option<String>,
        adapter: Option<u32>,
        seconds: u32,
        output: String,
    ) -> Self {
        Self {
            device_name,
            adapter,
            delivery_system: tp.delivery_system.to_string(),
            frequency_hz: tp.frequency_mhz as u64 * 1_000_000,
            polarity: Some(tp.polarity.tsduck().to_string()),
            symbol_rate: Some(tp.symbol_rate_ksps as u64 * 1_000),
            modulation: Some(tp.modulation.to_string()),
            fec: Some(tp.fec.to_string()),
            lnb: None,
            satellite_number: None,
            bandwidth_hz: None,
            guard_interval: None,
            plp: None,
            dvb_status_interval_s: None,
            signal_timeout_s: 10,
            seconds,
            output,
            live_analyze: false,
        }
    }
}

/// DTU-315 playout delivery family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayoutDelivery {
    Satellite,
    Terrestrial,
    Cable,
}

/// Parameters for a Dektec playout (transport-stream file -> modulator).
#[derive(Debug, Clone)]
pub struct PlayParams {
    /// Input transport-stream file path.
    pub input: String,
    /// Loop the file forever (useful as a continuous loopback source).
    pub infinite: bool,
    pub delivery: PlayoutDelivery,
    /// Output carrier frequency in Hz (L-band IF for DVB-S/S2; RF for DVB-T/C).
    pub frequency_hz: u64,
    /// Dektec modulation token, e.g. "DVB-S2-8PSK", "DVB-T2", "256-QAM".
    pub modulation: String,
    /// Symbol rate in symbols/s (satellite and cable).
    pub symbol_rate: Option<u64>,
    /// Convolutional/code rate for DVB-S/S2, or PLP code rate for DVB-T2.
    pub code_rate: Option<String>,
    /// Channel bandwidth in Hz (terrestrial).
    pub bandwidth_hz: Option<u64>,
    /// Guard interval for DVB-T playout (DVB-T2 uses `--t2-guard-interval`).
    pub guard_interval: Option<String>,
    /// DVB-T constellation / DVB-T2 PLP#0 modulation (e.g. `64-QAM`).
    pub plp_modulation: Option<String>,
    /// J.83 annex for DVB-C (`A`, `B`, or `C`).
    pub j83: Option<String>,
    /// Output level in units of 0.1 dBm (e.g. -300 = -30.0 dBm), if set.
    pub level_dbm_tenths: Option<i32>,
    /// Detach the Dektec output and let `tsp` exit immediately after preloading
    /// the FIFO. Useful for a fire-and-forget finite playout (avoids an
    /// end-of-stream hang on some devices), but MUST be `false` for a sustained
    /// loopback source: with detach on, `tsp` exits and transmission stops once
    /// the small on-board FIFO drains, so the receiver never stays locked.
    pub instant_detach: bool,
}

impl PlayParams {
    /// Builds the `tsp` argument vector for this playout.
    pub fn to_tsp_args(&self) -> Vec<String> {
        let mut a: Vec<String> = vec!["-I".into(), "file".into(), self.input.clone()];
        if self.infinite {
            a.push("--infinite".into());
        }
        a.push("-O".into());
        a.push("dektec".into());
        a.push("-f".into());
        a.push(self.frequency_hz.to_string());
        a.push("-m".into());
        a.push(self.modulation.clone());
        match self.delivery {
            PlayoutDelivery::Satellite => {
                if let Some(sr) = self.symbol_rate {
                    a.push("--symbol-rate".into());
                    a.push(sr.to_string());
                }
                if let Some(cr) = &self.code_rate {
                    a.push("-r".into());
                    a.push(cr.clone());
                }
            }
            PlayoutDelivery::Cable => {
                if let Some(sr) = self.symbol_rate {
                    a.push("--symbol-rate".into());
                    a.push(sr.to_string());
                }
                if let Some(j) = &self.j83 {
                    a.push("--j83".into());
                    a.push(j.clone());
                }
            }
            PlayoutDelivery::Terrestrial => {
                if let Some(bw) = self.bandwidth_hz {
                    a.push("--bandwidth".into());
                    a.push(dektec_bandwidth_mhz(bw));
                }
                if self.modulation.eq_ignore_ascii_case("DVB-T") {
                    if let Some(gi) = &self.guard_interval {
                        a.push("--guard-interval".into());
                        a.push(gi.clone());
                    }
                    if let Some(c) = &self.plp_modulation {
                        a.push("--constellation".into());
                        a.push(c.clone());
                    }
                    if let Some(cr) = &self.code_rate {
                        a.push("-r".into());
                        a.push(cr.clone());
                    }
                } else if self.modulation.eq_ignore_ascii_case("DVB-T2") {
                    append_dvbt2_dektec_args(&mut a, self);
                }
                a.push("--stuffing".into());
            }
        }
        if let Some(l) = self.level_dbm_tenths {
            a.push("--level".into());
            a.push(l.to_string());
        }
        if self.instant_detach {
            a.push("--instant-detach".into());
        }
        a
    }
}

/// Dektec `-O dektec --bandwidth` expects MHz (`8`), not Hz (`8000000`).
fn dektec_bandwidth_mhz(bw_hz: u64) -> String {
    let mhz = bw_hz / 1_000_000;
    match mhz {
        5 => "5".into(),
        6 => "6".into(),
        7 => "7".into(),
        8 => "8".into(),
        10 => "10".into(),
        1 | 2 => "1.7".into(),
        _ => mhz.to_string(),
    }
}

/// DTU-315 DVB-T2 playout flags (Istanbul / RTÜK plan: 32K, GI 19/256, 64-QAM).
fn append_dvbt2_dektec_args(a: &mut Vec<String>, params: &PlayParams) {
    a.push("--fft-mode".into());
    a.push("32K".into());
    if let Some(gi) = &params.guard_interval {
        a.push("--t2-guard-interval".into());
        a.push(gi.clone());
    }
    a.push("--t2-fpsf".into());
    a.push("2".into());
    a.push("--pilot-pattern".into());
    a.push("4".into());
    a.push("--plp0-type".into());
    a.push("1".into());
    if let Some(m) = &params.plp_modulation {
        a.push("--plp0-modulation".into());
        a.push(m.clone());
    }
    if let Some(cr) = &params.code_rate {
        a.push("--plp0-code-rate".into());
        a.push(cr.clone());
    }
    a.push("--plp0-fec-type".into());
    a.push("64K".into());
    a.push("--papr".into());
    a.push("NONE".into());
    a.push("--miso".into());
    a.push("OFF".into());
}

/// L-band IF (MHz) for a Ku downlink frequency using a universal LNB model.
pub fn l_band_if_mhz_universal(downlink_mhz: u32) -> u32 {
    const SPLIT: u32 = 11700;
    const LO_LOW: u32 = 9750;
    const LO_HIGH: u32 = 10600;
    if downlink_mhz >= SPLIT {
        downlink_mhz.saturating_sub(LO_HIGH)
    } else {
        downlink_mhz.saturating_sub(LO_LOW)
    }
}

/// Dektec `-m` token from a capture [`TuneRecord`].
pub fn dektec_modulation_from_tune(tune: &TuneRecord) -> Option<String> {
    let sys = tune.delivery_system.to_ascii_uppercase();
    let modu = tune.modulation.to_ascii_uppercase();
    let token = match sys.as_str() {
        "DVB-T2" => "DVB-T2",
        "DVB-T" => "DVB-T",
        "DVB-C" | "DVB-C/A" | "DVB-C/B" | "DVB-C/C" => match modu.as_str() {
            "256-QAM" | "256QAM" => "256-QAM",
            "64-QAM" | "64QAM" => "64-QAM",
            "128-QAM" | "128QAM" => "128-QAM",
            "32-QAM" | "32QAM" => "32-QAM",
            "16-QAM" | "16QAM" => "16-QAM",
            _ if modu.contains("256") => "256-QAM",
            _ if modu.contains("64") => "64-QAM",
            _ => return None,
        },
        _ => match (sys.as_str(), modu.as_str()) {
            ("DVB-S2", "8-PSK" | "8PSK") => "DVB-S2-8PSK",
            ("DVB-S2", "QPSK") => "DVB-S2-QPSK",
            ("DVB-S", "QPSK") => "DVB-S-QPSK",
            ("DVB-S", _) if modu.contains("QPSK") => "DVB-S-QPSK",
            _ => return None,
        },
    };
    Some(token.into())
}

/// J.83 annex for DVB-C sidecars (`DVB-C/A` → `A`).
pub fn j83_from_tune(tune: &TuneRecord) -> Option<String> {
    let sys = tune.delivery_system.to_ascii_uppercase();
    if sys == "DVB-C" {
        return Some("A".into());
    }
    sys.strip_prefix("DVB-C/")
        .map(|suffix| suffix.chars().next().unwrap_or('A').to_string())
}

/// Builds DTU-315 playout parameters from a capture sidecar tune block.
pub fn play_params_from_tune(
    input: String,
    tune: &TuneRecord,
    infinite: bool,
) -> anyhow::Result<PlayParams> {
    let modulation = dektec_modulation_from_tune(tune).ok_or_else(|| {
        anyhow::anyhow!(
            "unsupported tune for playout: {} {}",
            tune.delivery_system,
            tune.modulation
        )
    })?;
    let sys = tune.delivery_system.to_ascii_uppercase();
    if is_terrestrial_delivery(&sys) {
        return Ok(PlayParams {
            input,
            infinite,
            delivery: PlayoutDelivery::Terrestrial,
            frequency_hz: tune.frequency_mhz as u64 * 1_000_000,
            modulation,
            symbol_rate: None,
            code_rate: if tune.fec.is_empty() || tune.fec == "auto" {
                None
            } else {
                Some(tune.fec.clone())
            },
            bandwidth_hz: Some(8_000_000),
            guard_interval: Some("19/256".into()),
            plp_modulation: if tune.modulation.is_empty() {
                Some("64-QAM".into())
            } else {
                Some(tune.modulation.clone())
            },
            j83: None,
            level_dbm_tenths: None,
            instant_detach: !infinite,
        });
    }
    if sys.starts_with("DVB-C") {
        return Ok(PlayParams {
            input,
            infinite,
            delivery: PlayoutDelivery::Cable,
            frequency_hz: tune.frequency_mhz as u64 * 1_000_000,
            modulation,
            symbol_rate: Some(tune.symbol_rate_ksps as u64 * 1_000),
            code_rate: None,
            bandwidth_hz: None,
            guard_interval: None,
            plp_modulation: None,
            j83: j83_from_tune(tune).or(Some("A".into())),
            level_dbm_tenths: None,
            instant_detach: !infinite,
        });
    }
    let if_mhz = l_band_if_mhz_universal(tune.frequency_mhz);
    Ok(PlayParams {
        input,
        infinite,
        delivery: PlayoutDelivery::Satellite,
        frequency_hz: if_mhz as u64 * 1_000_000,
        modulation,
        symbol_rate: Some(tune.symbol_rate_ksps as u64 * 1_000),
        code_rate: Some(tune.fec.clone()),
        bandwidth_hz: None,
        guard_interval: None,
        plp_modulation: None,
        j83: None,
        level_dbm_tenths: None,
        instant_detach: !infinite,
    })
}

/// True for over-the-air delivery systems where TSDuck must not receive FEC.
pub fn is_terrestrial_delivery(system: &str) -> bool {
    let s = system.to_ascii_uppercase();
    s.starts_with("DVB-T")
        || s.starts_with("ISDB-T")
        || s.starts_with("ATSC")
        || s == "DTMB"
        || s == "CMMB"
}

/// Maps a Dektec modulation token back to (delivery_system, dvb modulation) so
/// the capture side of a loopback can be tuned consistently.
pub fn dvb_from_dektec_modulation(token: &str) -> Option<(&'static str, &'static str)> {
    match token {
        "DVB-S2-8PSK" => Some(("DVB-S2", "8-PSK")),
        "DVB-S2-QPSK" => Some(("DVB-S2", "QPSK")),
        "DVB-S-QPSK" | "DVB-S" => Some(("DVB-S", "QPSK")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze::TuneRecord;

    #[test]
    fn play_params_from_dvbt2_sidecar() {
        let tune = TuneRecord {
            frequency_mhz: 618,
            polarity: String::new(),
            symbol_rate_ksps: 0,
            delivery_system: "DVB-T2".into(),
            modulation: "64-QAM".into(),
            fec: "2/3".into(),
            ..Default::default()
        };
        let params =
            play_params_from_tune("/tmp/x.ts".into(), &tune, true).expect("DVB-T2 must work");
        assert_eq!(params.delivery, PlayoutDelivery::Terrestrial);
        assert_eq!(params.frequency_hz, 618_000_000);
        assert_eq!(params.modulation, "DVB-T2");
        let args = params.to_tsp_args().join(" ");
        assert!(args.contains("-f 618000000"));
        assert!(args.contains("-m DVB-T2"));
        assert!(args.contains("--bandwidth 8"));
        assert!(!args.contains("--bandwidth 8000000"));
        assert!(args.contains("--t2-guard-interval 19/256"));
        assert!(args.contains("--plp0-modulation 64-QAM"));
        assert!(args.contains("--plp0-code-rate 2/3"));
        assert!(args.contains("--plp0-fec-type 64K"));
        assert!(args.contains("--fft-mode 32K"));
        assert!(args.contains("--stuffing"));
        assert!(!args.contains("--symbol-rate"));
        assert!(!args.contains("-r "));
    }
}

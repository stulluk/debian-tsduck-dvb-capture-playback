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
        if let Some(f) = &self.fec {
            a.push("--fec-inner".into());
            a.push(f.clone());
        }
        if let Some(l) = &self.lnb {
            a.push("--lnb".into());
            a.push(l.clone());
        }
        if let Some(sn) = self.satellite_number {
            a.push("--satellite-number".into());
            a.push(sn.to_string());
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
            signal_timeout_s: 10,
            seconds,
            output,
            live_analyze: false,
        }
    }
}

/// Parameters for a Dektec playout (transport-stream file -> modulator).
#[derive(Debug, Clone)]
pub struct PlayParams {
    /// Input transport-stream file path.
    pub input: String,
    /// Loop the file forever (useful as a continuous loopback source).
    pub infinite: bool,
    /// Output carrier frequency in Hz (L-band IF for DVB-S/S2: 950-2150 MHz).
    pub frequency_hz: u64,
    /// Dektec modulation token, e.g. "DVB-S2-8PSK".
    pub modulation: String,
    /// Symbol rate in symbols/s.
    pub symbol_rate: u64,
    /// Convolutional/code rate, e.g. "3/4".
    pub code_rate: String,
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
        a.push("--symbol-rate".into());
        a.push(self.symbol_rate.to_string());
        a.push("-r".into());
        a.push(self.code_rate.clone());
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
    let token = match (sys.as_str(), modu.as_str()) {
        ("DVB-S2", "8-PSK" | "8PSK") => "DVB-S2-8PSK",
        ("DVB-S2", "QPSK") => "DVB-S2-QPSK",
        ("DVB-S", "QPSK") => "DVB-S-QPSK",
        ("DVB-S", _) if modu.contains("QPSK") => "DVB-S-QPSK",
        _ => return None,
    };
    Some(token.into())
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
    let if_mhz = l_band_if_mhz_universal(tune.frequency_mhz);
    Ok(PlayParams {
        input,
        infinite,
        frequency_hz: if_mhz as u64 * 1_000_000,
        modulation,
        symbol_rate: tune.symbol_rate_ksps as u64 * 1_000,
        code_rate: tune.fec.clone(),
        level_dbm_tenths: None,
        instant_detach: !infinite,
    })
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

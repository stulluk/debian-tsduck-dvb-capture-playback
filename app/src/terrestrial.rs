//! Terrestrial (DVB-T / DVB-T2) multiplex definitions and capture parameters.
//!
//! Istanbul presets are derived from public reports (RTÜK national plan parameters,
//! Çamlıca tower tests, and historical trial muxes). Turkey has no stable nationwide
//! DTT network yet; treat these as starting points for local reception tests.

use crate::pipeline::CaptureParams;

/// One terrestrial multiplex tuning target.
#[derive(Debug, Clone)]
pub struct TerrestrialMux {
    /// Short UI label.
    pub label: String,
    /// City / site note.
    pub location: String,
    /// UHF channel number when known (ITU mapping).
    pub uhf_channel: Option<u8>,
    /// Centre frequency in MHz.
    pub frequency_mhz: u32,
    /// `DVB-T` or `DVB-T2`.
    pub delivery_system: String,
    /// Channel bandwidth in MHz (typically 8 in Turkey).
    pub bandwidth_mhz: u8,
    /// TSDuck `--guard-interval` value.
    pub guard_interval: String,
    /// Optional `--modulation` (64-QAM is the RTÜK plan default for T2).
    pub modulation: Option<String>,
    /// Optional `--fec-inner`.
    pub fec: Option<String>,
    /// Physical layer pipe (`--plp`); `None` omits the flag (driver default).
    pub plp: Option<u8>,
    /// Where this preset came from (shown in TUI footer).
    pub source: String,
}

impl TerrestrialMux {
    /// Builds TSDuck capture parameters for the TBS5530 terrestrial frontend.
    pub fn to_capture_params(
        &self,
        device_name: &str,
        seconds: u32,
        signal_timeout_s: u32,
        output: String,
    ) -> CaptureParams {
        CaptureParams {
            device_name: Some(device_name.to_string()),
            adapter: None,
            delivery_system: self.delivery_system.clone(),
            frequency_hz: self.frequency_mhz as u64 * 1_000_000,
            polarity: None,
            symbol_rate: None,
            modulation: self.modulation.clone(),
            fec: self.fec.clone(),
            lnb: None,
            satellite_number: None,
            bandwidth_hz: Some(self.bandwidth_mhz as u64 * 1_000_000),
            guard_interval: Some(self.guard_interval.clone()),
            plp: self.plp,
            dvb_status_interval_s: Some(3),
            signal_timeout_s,
            seconds,
            output,
            live_analyze: true,
        }
    }

    /// One-line summary for the capture run queue.
    pub fn queue_label(&self) -> String {
        let ch = self
            .uhf_channel
            .map(|c| format!("CH{c} "))
            .unwrap_or_default();
        let plp = self
            .plp
            .map(|p| format!(" PLP{p}"))
            .unwrap_or_default();
        format!(
            "{} {} MHz {}{} {} MHz GI {}",
            ch,
            self.frequency_mhz,
            self.delivery_system,
            plp,
            self.bandwidth_mhz,
            self.guard_interval
        )
    }
}

/// Turkey RTÜK national DVB-T2 plan defaults (32K / 19/256 / 64-QAM / 2/3).
const PLAN_T2_MOD: &str = "64-QAM";
const PLAN_T2_FEC: &str = "2/3";
const PLAN_T2_GI: &str = "19/256";

/// Istanbul and national terrestrial presets for the capture TUI.
pub fn turkey_presets() -> Vec<TerrestrialMux> {
    vec![TerrestrialMux {
        label: "Istanbul CH39 — 618 MHz (Xiaomi TV / Kule mux)".into(),
        location: "Istanbul (UHF CH39, live bench reference)".into(),
        uhf_channel: Some(39),
        frequency_mhz: 618,
        delivery_system: "DVB-T2".into(),
        bandwidth_mhz: 8,
        guard_interval: PLAN_T2_GI.into(),
        modulation: Some(PLAN_T2_MOD.into()),
        fec: Some(PLAN_T2_FEC.into()),
        plp: Some(0),
        source: "Verified on TBS5530 via tsp; Xiaomi Android TV logcat (TGRT HD Kule, 618 MHz)".into(),
    }]
}

/// Directory slug for capture output (`514mhz`).
pub fn freq_dir_name(mhz: u32) -> String {
    format!("{mhz}mhz")
}

/// Base capture file stem: `terr_514_dvbt2_YYYYMMDD_HHMMSS`.
pub fn capture_base_name(mux: &TerrestrialMux) -> String {
    let sys = mux
        .delivery_system
        .to_lowercase()
        .replace('-', "");
    let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    format!("terr_{}_{sys}_{stamp}", mux.frequency_mhz)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_non_empty() {
        assert_eq!(turkey_presets().len(), 1);
    }

    #[test]
    fn capture_params_include_terrestrial_flags() {
        let mux = &turkey_presets()[0];
        let p = mux.to_capture_params("/dev/dvb/adapter1", 10, 15, "/tmp/x.ts".into());
        let args = p.to_tsp_args();
        let joined = args.join(" ");
        assert!(joined.contains("--bandwidth 8000000"));
        assert!(joined.contains("--guard-interval"));
        assert!(joined.contains("--plp"));
        assert!(joined.contains("--json-line"));
        assert!(joined.contains("--json-interval 3"));
        assert!(
            !joined.contains("--fec-inner"),
            "terrestrial capture must not pass --fec-inner (breaks DVB-T2 lock)"
        );
    }

    #[test]
    fn ch39_preset_is_618_mhz() {
        let mux = &turkey_presets()[0];
        assert_eq!(mux.frequency_mhz, 618);
        assert_eq!(mux.uhf_channel, Some(39));
    }
}

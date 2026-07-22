//! Cable (DVB-C) multiplex definitions and capture parameters.

use crate::pipeline::CaptureParams;

/// One DVB-C tuning target.
#[derive(Debug, Clone)]
pub struct CableMux {
    pub label: String,
    pub location: String,
    pub frequency_mhz: u32,
    pub symbol_rate_ksps: u32,
    /// TSDuck `--delivery-system` (e.g. `DVB-C`, `DVB-C/A`).
    pub delivery_system: String,
    pub modulation: String,
    pub fec: Option<String>,
    pub source: String,
}

impl CableMux {
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
            symbol_rate: Some(self.symbol_rate_ksps as u64 * 1_000),
            modulation: Some(self.modulation.clone()),
            fec: self.fec.clone(),
            lnb: None,
            satellite_number: None,
            bandwidth_hz: None,
            guard_interval: None,
            plp: None,
            dvb_status_interval_s: Some(3),
            signal_timeout_s,
            seconds,
            output,
            live_analyze: true,
        }
    }

    pub fn queue_label(&self) -> String {
        format!(
            "{} MHz {} SR {} kS/s {}",
            self.frequency_mhz, self.delivery_system, self.symbol_rate_ksps, self.modulation
        )
    }
}

/// Bench / loopback DVB-C presets.
pub fn bench_presets() -> Vec<CableMux> {
    vec![CableMux {
        label: "Vectra PL — 722 MHz (DTU-315 loopback)".into(),
        location: "Bench reference (Poland Vectra mux, DTU-315 → TBS5530)".into(),
        frequency_mhz: 722,
        symbol_rate_ksps: 6876,
        delivery_system: "DVB-C".into(),
        modulation: "256-QAM".into(),
        fec: Some("auto".into()),
        source: "Verified loopback; source TS: 722_Vectra_4h-DVBC-256QAM-8Mhz-BW-1Gbyte-resync.ts"
            .into(),
    }]
}

pub fn freq_dir_name(mhz: u32) -> String {
    format!("{mhz}mhz")
}

pub fn capture_base_name(mux: &CableMux) -> String {
    let sys = mux
        .delivery_system
        .to_lowercase()
        .replace(['/', '-'], "");
    let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    format!("cable_{}_{sys}_{stamp}", mux.frequency_mhz)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_722_vectra() {
        let mux = &bench_presets()[0];
        assert_eq!(mux.frequency_mhz, 722);
        assert_eq!(mux.symbol_rate_ksps, 6876);
    }

    #[test]
    fn capture_params_include_cable_flags() {
        let mux = &bench_presets()[0];
        let p = mux.to_capture_params("/dev/dvb/adapter1", 10, 15, "/tmp/x.ts".into());
        let args = p.to_tsp_args();
        let joined = args.join(" ");
        assert!(joined.contains("--delivery-system DVB-C"));
        assert!(joined.contains("--symbol-rate 6876000"));
        assert!(joined.contains("--modulation 256-QAM"));
        assert!(joined.contains("--json-line"));
    }
}

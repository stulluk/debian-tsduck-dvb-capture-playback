//! Persisted user databases: reference transponders and reference captures.
//!
//! * Reference transponders (`reference_transponders.json`) are TPs the user
//!   saved from the manual capture form ("save as reference transponder").
//!   They are seeded on first run from the built-in Turksat 42E set.
//! * Reference captures (`reference_captures.json`) are previously recorded TS
//!   files together with the modulation parameters used to play them back out
//!   through the DTU-315 ("save as reference capture").

use crate::satellites::SatTransponder;
use crate::transponders::{Polarity, REFERENCE_TRANSPONDERS};
use serde::{Deserialize, Serialize};

/// A user-managed list of reference transponders.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReferenceTpDb {
    pub transponders: Vec<SatTransponder>,
}

impl ReferenceTpDb {
    /// Loads the list from disk; seeds from the built-in reference set on first
    /// run and persists it.
    pub fn load() -> Self {
        let path = crate::config::reference_transponders_file_path();
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(db) = serde_json::from_str::<ReferenceTpDb>(&text) {
                return db;
            }
        }
        let db = Self::seed();
        let _ = db.save();
        db
    }

    /// Builds the default reference list from the built-in Turksat 42E TPs.
    pub fn seed() -> Self {
        let transponders = REFERENCE_TRANSPONDERS
            .iter()
            .map(|tp| SatTransponder {
                frequency_mhz: tp.frequency_mhz,
                polarity: match tp.polarity {
                    Polarity::Horizontal => "horizontal".to_string(),
                    Polarity::Vertical => "vertical".to_string(),
                },
                symbol_rate_ksps: tp.symbol_rate_ksps,
                modulation: tp.modulation.to_string(),
                fec: tp.fec.to_string(),
                delivery_system: tp.delivery_system.to_string(),
                name: tp.content.to_string(),
            })
            .collect();
        Self { transponders }
    }

    /// Persists the list to disk, creating parent directories as needed.
    pub fn save(&self) -> std::io::Result<()> {
        let path = crate::config::reference_transponders_file_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(&path, text)
    }

    /// Appends a transponder, de-duplicating by (freq, polarity, symbol rate).
    pub fn add(&mut self, tp: SatTransponder) {
        let dup = self.transponders.iter().any(|t| {
            t.frequency_mhz == tp.frequency_mhz
                && t.polarity == tp.polarity
                && t.symbol_rate_ksps == tp.symbol_rate_ksps
        });
        if !dup {
            self.transponders.push(tp);
        }
    }

    /// Removes the transponder at `index`; returns true if it existed.
    pub fn remove(&mut self, index: usize) -> bool {
        if index < self.transponders.len() {
            self.transponders.remove(index);
            true
        } else {
            false
        }
    }
}

/// A reference playout capture: a TS file plus its modulation parameters.
/// Also the body of the web playout request. Older entries (satellite only)
/// still load: the newer fields default.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReferenceCapture {
    /// User-facing label (empty for an ad-hoc playout).
    #[serde(default)]
    pub name: String,
    /// Path to the transport-stream file (absolute, or relative to the capture dir).
    pub file: String,
    /// "satellite" (default), "terrestrial" or "cable".
    #[serde(default = "default_delivery")]
    pub delivery: String,
    /// Output frequency in MHz: L-band IF for satellite, RF for terrestrial/cable.
    pub frequency_mhz: u32,
    /// Dektec modulation token, e.g. "DVB-S2-8PSK", "DVB-T2", "256-QAM".
    pub modulation: String,
    /// Symbol rate in kSymbols/s (satellite, cable).
    #[serde(default)]
    pub symbol_rate_ksps: u32,
    /// Code rate, e.g. "3/4" (satellite; DVB-T2 PLP code rate).
    #[serde(default)]
    pub code_rate: String,
    /// Terrestrial channel bandwidth in MHz.
    #[serde(default)]
    pub bandwidth_mhz: Option<u8>,
    /// Terrestrial guard interval (e.g. "1/8", "19/256").
    #[serde(default)]
    pub guard_interval: Option<String>,
    /// DVB-T constellation / DVB-T2 PLP modulation (e.g. "64-QAM").
    #[serde(default)]
    pub plp_modulation: Option<String>,
    /// DVB-C J.83 annex ("A", "B", "C").
    #[serde(default)]
    pub j83: Option<String>,
    /// Output level in 0.1 dBm units, if set.
    #[serde(default)]
    pub level: Option<i32>,
    /// Loop the file forever.
    #[serde(default)]
    pub infinite: bool,
}

fn default_delivery() -> String {
    "satellite".into()
}

impl ReferenceCapture {
    /// Dektec playout parameters for `input` (already resolved to a full path).
    pub fn to_play_params(&self, input: String) -> anyhow::Result<crate::pipeline::PlayParams> {
        use crate::pipeline::{PlayParams, PlayoutDelivery};
        let delivery = match self.delivery.to_ascii_lowercase().as_str() {
            "satellite" | "sat" | "" => PlayoutDelivery::Satellite,
            "terrestrial" | "ter" => PlayoutDelivery::Terrestrial,
            "cable" | "cab" => PlayoutDelivery::Cable,
            other => anyhow::bail!("unknown delivery {other:?}"),
        };
        anyhow::ensure!(self.frequency_mhz > 0, "frequency_mhz is required");
        anyhow::ensure!(!self.modulation.trim().is_empty(), "modulation is required");
        let nonempty = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_string());
        let sr = (self.symbol_rate_ksps > 0).then(|| u64::from(self.symbol_rate_ksps) * 1_000);
        Ok(PlayParams {
            input,
            infinite: self.infinite,
            frequency_hz: u64::from(self.frequency_mhz) * 1_000_000,
            modulation: self.modulation.clone(),
            symbol_rate: match delivery {
                PlayoutDelivery::Terrestrial => None,
                _ => sr,
            },
            code_rate: nonempty(&self.code_rate),
            bandwidth_hz: match delivery {
                PlayoutDelivery::Terrestrial => {
                    Some(u64::from(self.bandwidth_mhz.unwrap_or(8)) * 1_000_000)
                }
                _ => None,
            },
            guard_interval: self.guard_interval.clone(),
            plp_modulation: self.plp_modulation.clone(),
            j83: match delivery {
                PlayoutDelivery::Cable => Some(self.j83.clone().unwrap_or_else(|| "A".into())),
                _ => None,
            },
            level_dbm_tenths: self.level,
            // Stay attached while looping (a detached tsp stops after the FIFO drains).
            instant_detach: !self.infinite,
            delivery,
        })
    }

    /// Form values derived from a capture sidecar (`*_services.json` tune block).
    pub fn from_tune(file: String, tune: &crate::analyze::TuneRecord) -> anyhow::Result<Self> {
        use crate::pipeline::PlayoutDelivery;
        let p = crate::pipeline::play_params_from_tune(file.clone(), tune, true)?;
        Ok(Self {
            name: String::new(),
            file,
            delivery: match p.delivery {
                PlayoutDelivery::Satellite => "satellite",
                PlayoutDelivery::Terrestrial => "terrestrial",
                PlayoutDelivery::Cable => "cable",
            }
            .into(),
            frequency_mhz: (p.frequency_hz / 1_000_000) as u32,
            modulation: p.modulation,
            symbol_rate_ksps: p.symbol_rate.map(|s| (s / 1_000) as u32).unwrap_or(0),
            code_rate: p.code_rate.unwrap_or_default(),
            bandwidth_mhz: p.bandwidth_hz.map(|b| (b / 1_000_000) as u8),
            guard_interval: p.guard_interval,
            plp_modulation: p.plp_modulation,
            j83: p.j83,
            level: None,
            infinite: true,
        })
    }
}

/// A user-managed list of reference captures.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReferenceCaptureDb {
    pub captures: Vec<ReferenceCapture>,
}

impl ReferenceCaptureDb {
    /// Loads the list from disk (empty if missing/invalid).
    pub fn load() -> Self {
        let path = crate::config::reference_captures_file_path();
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(db) = serde_json::from_str::<ReferenceCaptureDb>(&text) {
                return db;
            }
        }
        Self::default()
    }

    /// Persists the list to disk, creating parent directories as needed.
    pub fn save(&self) -> std::io::Result<()> {
        let path = crate::config::reference_captures_file_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(&path, text)
    }

    /// Adds or replaces a capture (matched by name).
    pub fn upsert(&mut self, cap: ReferenceCapture) {
        match self
            .captures
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(&cap.name))
        {
            Some(i) => self.captures[i] = cap,
            None => self.captures.push(cap),
        }
    }

    /// Removes a capture by name; returns true if one was removed.
    pub fn remove(&mut self, name: &str) -> bool {
        match self
            .captures
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(name))
        {
            Some(i) => {
                self.captures.remove(i);
                true
            }
            None => false,
        }
    }
}

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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferenceCapture {
    /// User-facing label.
    pub name: String,
    /// Path to the transport-stream file.
    pub file: String,
    /// Output L-band IF frequency in MHz.
    pub frequency_mhz: u32,
    /// Dektec modulation token, e.g. "DVB-S2-8PSK".
    pub modulation: String,
    /// Symbol rate in kSymbols/s.
    pub symbol_rate_ksps: u32,
    /// Convolutional/code rate, e.g. "3/4".
    pub code_rate: String,
    /// Output level in 0.1 dBm units, if set.
    #[serde(default)]
    pub level: Option<i32>,
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

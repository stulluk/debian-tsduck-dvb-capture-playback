//! Satellite / transponder database with KingOfSat import.
//!
//! The web UI lets the user manage satellites and their transponders the way a
//! consumer receiver does (name, longitude, LNB, DiSEqC). The catalogue is
//! persisted to `<data_dir>/satellites.json`. On first run it is seeded from
//! bundled KingOfSat snapshots (Turksat 42E, Hot Bird 13E, Astra 19.2E) so the
//! user always has a populated database offline; it can be refreshed live from
//! KingOfSat at runtime.

use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

/// Bundled KingOfSat snapshots used to seed the database on first run.
const SEED_42E: &str = include_str!("../data/kingofsat/0420.ini");
const SEED_13E: &str = include_str!("../data/kingofsat/0130.ini");
const SEED_192E: &str = include_str!("../data/kingofsat/0192.ini");

/// A transponder definition (owned and persistable).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SatTransponder {
    /// Downlink frequency in MHz.
    pub frequency_mhz: u32,
    /// Polarity: "vertical", "horizontal", "left", "right".
    pub polarity: String,
    /// Symbol rate in kSymbols/s.
    pub symbol_rate_ksps: u32,
    /// Modulation as a TSDuck value ("QPSK", "8-PSK", "16-APSK", "32-APSK").
    pub modulation: String,
    /// Inner FEC ("3/4", "5/6", "auto", ...).
    pub fec: String,
    /// Delivery system ("DVB-S", "DVB-S2").
    pub delivery_system: String,
    /// Optional content/label note.
    #[serde(default)]
    pub name: String,
}

/// Default LNB token when none is specified.
fn default_lnb() -> String {
    "universal".to_string()
}

/// A satellite entry, mirroring a consumer receiver's satellite definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Satellite {
    /// User-facing name, e.g. "Turksat 42E".
    pub name: String,
    /// Orbital position label, e.g. "42.0E" (also the KingOfSat `pos` code).
    pub longitude: String,
    /// LNB token: "universal" (Ku auto band/tone) or "0" (direct L-band).
    #[serde(default = "default_lnb")]
    pub lnb: String,
    /// DiSEqC / satellite-number port (0-63); 0 = port A.
    #[serde(default)]
    pub diseqc: u32,
    /// Transponders belonging to this satellite.
    #[serde(default)]
    pub transponders: Vec<SatTransponder>,
}

impl Satellite {
    /// Value for `tsp -I dvb --lnb`.
    ///
    /// The catalogue token "universal" means a standard Ku universal LNB
    /// (9750/10600 MHz, switch 11700, 10.7-12.75 GHz). In TSDuck the name
    /// "Universal" is the *old* variant whose low band starts at 10.8 GHz, so
    /// it rejects TPs such as Hot Bird 10719. Map it to TSDuck's "Extended".
    pub fn tsp_lnb(&self) -> String {
        tsp_lnb_value(&self.lnb)
    }
}

/// Maps a catalogue LNB token to a `tsp --lnb` value ("universal" → "Extended").
pub fn tsp_lnb_value(token: &str) -> String {
    if token.eq_ignore_ascii_case("universal") {
        "Extended".to_string()
    } else {
        token.to_string()
    }
}

/// The persisted satellite catalogue.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SatelliteDb {
    pub satellites: Vec<Satellite>,
}

impl SatelliteDb {
    /// Loads the database from disk; on first run, seeds it from the bundled
    /// KingOfSat snapshots and persists it.
    pub fn load() -> Self {
        let path = crate::config::satellites_file_path();
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(db) = serde_json::from_str::<SatelliteDb>(&text) {
                return db;
            }
        }
        let db = Self::seed();
        let _ = db.save();
        db
    }

    /// Builds the default catalogue from the bundled KingOfSat snapshots.
    pub fn seed() -> Self {
        let mut satellites = Vec::new();
        for (raw, name, lon) in [
            (SEED_42E, "Turksat 42E", "42.0E"),
            (SEED_13E, "Hot Bird 13E", "13.0E"),
            (SEED_192E, "Astra 19.2E", "19.2E"),
        ] {
            let (_label, tps) = parse_kingofsat_ini(raw);
            satellites.push(Satellite {
                name: name.to_string(),
                longitude: lon.to_string(),
                lnb: default_lnb(),
                diseqc: 0,
                transponders: tps,
            });
        }
        Self { satellites }
    }

    /// Persists the database to disk, creating parent directories as needed.
    pub fn save(&self) -> std::io::Result<()> {
        let path = crate::config::satellites_file_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(&path, text)
    }

    /// Returns the index of a satellite by name (case-insensitive).
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.satellites
            .iter()
            .position(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// Adds or replaces a satellite (matched by name).
    pub fn upsert(&mut self, sat: Satellite) {
        match self.index_of(&sat.name) {
            Some(i) => self.satellites[i] = sat,
            None => self.satellites.push(sat),
        }
    }

    /// Removes a satellite by name; returns true if one was removed.
    pub fn remove(&mut self, name: &str) -> bool {
        match self.index_of(name) {
            Some(i) => {
                self.satellites.remove(i);
                true
            }
            None => false,
        }
    }

    /// Refreshes transponder lists from KingOfSat for all (or one) catalogue entries.
    ///
    /// Per-satellite `name`, `longitude`, `lnb`, and `diseqc` are preserved; only
    /// `transponders` is replaced with the live KingOfSat data.
    pub fn refresh_from_kingofsat(
        &mut self,
        only_name: Option<&str>,
    ) -> anyhow::Result<Vec<RefreshReport>> {
        let mut reports = Vec::new();
        for sat in &mut self.satellites {
            if let Some(filter) = only_name {
                if !sat.name.eq_ignore_ascii_case(filter) {
                    continue;
                }
            }
            let pos = kingofsat_pos_from_longitude(&sat.longitude);
            let (label, tps) = fetch_position(&pos)
                .with_context(|| format!("{} ({pos})", sat.name))?;
            let report = RefreshReport {
                name: sat.name.clone(),
                pos,
                kos_label: label,
                old_count: sat.transponders.len(),
                new_count: tps.len(),
            };
            sat.transponders = tps;
            reports.push(report);
        }
        if let Some(filter) = only_name {
            if reports.is_empty() {
                anyhow::bail!("satellite not found in catalogue: {filter}");
            }
        }
        if reports.is_empty() {
            anyhow::bail!("satellite catalogue is empty — nothing to refresh");
        }
        Ok(reports)
    }
}

/// Summary line for one satellite after a KingOfSat refresh.
#[derive(Debug, Clone)]
pub struct RefreshReport {
    pub name: String,
    pub pos: String,
    pub kos_label: String,
    pub old_count: usize,
    pub new_count: usize,
}

/// Maps a stored longitude label to a KingOfSat `pos` query token.
///
/// Examples: `42.0E` → `42E`, `13.0E` → `13E`, `19.2E` unchanged.
pub fn kingofsat_pos_from_longitude(longitude: &str) -> String {
    let s = longitude.trim();
    if s.ends_with(".0E") {
        return format!("{}E", &s[..s.len() - 3]);
    }
    if s.ends_with(".0e") {
        return format!("{}e", &s[..s.len() - 3]);
    }
    s.to_string()
}

/// Parses a KingOfSat `dl.php` ini payload into transponders.
///
/// The format (one transponder per line in the `[DVB]` section) is:
/// `idx=freq_mhz,pol,symrate_ksps,fec,delivery,modulation`, e.g.
/// `22=11054,V,30000,34,S2,8PSK`. The `0=<count>` line and comments are
/// skipped. Returns the satellite label (from `[SATTYPE] 2=...`) and TPs.
pub fn parse_kingofsat_ini(text: &str) -> (String, Vec<SatTransponder>) {
    let mut label = String::new();
    let mut tps = Vec::new();
    let mut section = "";

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = match &line[1..line.len() - 1] {
                "SATTYPE" => "sattype",
                "DVB" => "dvb",
                _ => "",
            };
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match section {
            "sattype" if key == "2" => label = value.trim().to_string(),
            "dvb" => {
                // `0=<count>` is a header, not a transponder.
                if key == "0" {
                    continue;
                }
                if let Some(tp) = parse_kingofsat_tp(value) {
                    tps.push(tp);
                }
            }
            _ => {}
        }
    }
    (label, tps)
}

/// Parses a single KingOfSat transponder field tuple.
fn parse_kingofsat_tp(value: &str) -> Option<SatTransponder> {
    let f: Vec<&str> = value.split(',').map(|s| s.trim()).collect();
    if f.len() < 6 {
        return None;
    }
    let frequency_mhz: u32 = f[0].parse().ok()?;
    let symbol_rate_ksps: u32 = f[2].parse().ok()?;
    Some(SatTransponder {
        frequency_mhz,
        polarity: map_polarity(f[1]).to_string(),
        symbol_rate_ksps,
        modulation: map_modulation(f[5]).to_string(),
        fec: map_fec(f[3]),
        delivery_system: map_delivery(f[4]).to_string(),
        name: String::new(),
    })
}

/// Maps a KingOfSat polarity letter to a TSDuck polarity value.
fn map_polarity(p: &str) -> &'static str {
    match p.to_ascii_uppercase().as_str() {
        "H" => "horizontal",
        "V" => "vertical",
        "L" => "left",
        "R" => "right",
        _ => "horizontal",
    }
}

/// Maps a KingOfSat delivery token to a TSDuck delivery-system value.
fn map_delivery(d: &str) -> &'static str {
    match d.to_ascii_uppercase().as_str() {
        "S2" | "DVB-S2" => "DVB-S2",
        _ => "DVB-S",
    }
}

/// Maps a KingOfSat modulation token to a TSDuck modulation value.
fn map_modulation(m: &str) -> &'static str {
    match m.to_ascii_uppercase().as_str() {
        "QPSK" => "QPSK",
        "8PSK" => "8-PSK",
        "16APSK" => "16-APSK",
        "32APSK" => "32-APSK",
        "64APSK" => "64-APSK",
        _ => "QPSK",
    }
}

/// Maps a KingOfSat FEC code (e.g. "34") to a TSDuck FEC value ("3/4").
fn map_fec(code: &str) -> String {
    let mapped = match code {
        "12" => "1/2",
        "13" => "1/3",
        "14" => "1/4",
        "23" => "2/3",
        "25" => "2/5",
        "34" => "3/4",
        "35" => "3/5",
        "45" => "4/5",
        "56" => "5/6",
        "67" => "6/7",
        "78" => "7/8",
        "89" => "8/9",
        "910" => "9/10",
        "Nn" | "" => "auto",
        other => other,
    };
    mapped.to_string()
}

/// Fetches a satellite position live from KingOfSat and parses its TPs.
///
/// `pos` is the KingOfSat position code (e.g. "42E", "13E", "19.2E").
pub fn fetch_position(pos: &str) -> anyhow::Result<(String, Vec<SatTransponder>)> {
    let url = format!("https://en.kingofsat.net/dl.php?pos={pos}&fkhz=0");
    let body = ureq::get(&url)
        .timeout(Duration::from_secs(20))
        .set("User-Agent", "dvbhub")
        .call()
        .with_context(|| format!("KingOfSat request failed for pos {pos}"))?
        .into_string()
        .context("KingOfSat response was not valid UTF-8")?;
    let (label, tps) = parse_kingofsat_ini(&body);
    if tps.is_empty() {
        anyhow::bail!("KingOfSat returned no transponders for pos {pos}");
    }
    Ok((label, tps))
}

/// Reads a bundled or on-disk ini file path into TPs (used for tests/tools).
#[allow(dead_code)]
pub fn parse_ini_file(path: &Path) -> anyhow::Result<(String, Vec<SatTransponder>)> {
    let text = std::fs::read_to_string(path)?;
    Ok(parse_kingofsat_ini(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn universal_lnb_maps_to_tsduck_extended() {
        let mut sat = Satellite {
            name: "Hot Bird 13E".into(),
            longitude: "13.0E".into(),
            lnb: "universal".into(),
            diseqc: 0,
            transponders: Vec::new(),
        };
        assert_eq!(sat.tsp_lnb(), "Extended");
        sat.lnb = "0".into();
        assert_eq!(sat.tsp_lnb(), "0");
        sat.lnb = "9750,10600,11700".into();
        assert_eq!(sat.tsp_lnb(), "9750,10600,11700");
    }

    #[test]
    fn kingofsat_pos_normalizes_trailing_zero() {
        assert_eq!(kingofsat_pos_from_longitude("42.0E"), "42E");
        assert_eq!(kingofsat_pos_from_longitude("13.0E"), "13E");
        assert_eq!(kingofsat_pos_from_longitude("19.2E"), "19.2E");
    }

    #[test]
    fn parses_seed_42e() {
        let (label, tps) = parse_kingofsat_ini(SEED_42E);
        assert_eq!(label, "42E");
        assert!(tps.len() > 100);
        // 22=11054,V,30000,34,S2,8PSK
        let trt = tps.iter().find(|t| t.frequency_mhz == 11054).unwrap();
        assert_eq!(trt.polarity, "vertical");
        assert_eq!(trt.symbol_rate_ksps, 30000);
        assert_eq!(trt.modulation, "8-PSK");
        assert_eq!(trt.fec, "3/4");
        assert_eq!(trt.delivery_system, "DVB-S2");
    }
}

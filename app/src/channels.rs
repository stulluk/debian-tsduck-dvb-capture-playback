//! Persisted TV channel catalogue (scan results the user chose to keep).
//!
//! Channels are stored in `<data_dir>/channels.json` and listed in scan order.
//! Each entry carries enough tuning metadata to play the service back without
//! re-entering transponder parameters.

use serde::{Deserialize, Serialize};

/// Delivery category for a saved channel.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChannelDelivery {
    #[default]
    Satellite,
    Terrestrial,
    Cable,
}

impl ChannelDelivery {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Satellite => "satellite",
            Self::Terrestrial => "terrestrial",
            Self::Cable => "cable",
        }
    }
}

/// A user-saved channel discovered during scanning.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedChannel {
    pub id: u64,
    pub delivery: ChannelDelivery,
    pub name: String,
    pub service_id: u64,
    pub provider: String,
    pub scrambled: bool,
    /// Order in which the channel was saved (lower = earlier in the list).
    pub scan_index: u32,
    /// Tuner path at scan time. Informational only: adapter numbers change
    /// with plug order, so playback picks a tuner by delivery (+ model).
    pub device_name: Option<String>,
    /// Model of the tuner that found the channel (e.g. "TBS5530"); preferred on playback.
    #[serde(default)]
    pub tuner_model: Option<String>,
    /// Satellite catalogue name (satellite channels).
    #[serde(default)]
    pub satellite: Option<String>,
    /// `tsp --lnb` value (satellite channels; `None` = TSDuck default "Extended").
    #[serde(default)]
    pub lnb: Option<String>,
    pub delivery_system: Option<String>,
    pub frequency_mhz: Option<u32>,
    pub polarity: Option<String>,
    pub symbol_rate_ksps: Option<u32>,
    pub modulation: Option<String>,
    pub fec: Option<String>,
    pub satellite_number: Option<u32>,
    /// Terrestrial bandwidth in MHz (watch/capture).
    #[serde(default)]
    pub bandwidth_mhz: Option<u8>,
    #[serde(default)]
    pub guard_interval: Option<String>,
    #[serde(default)]
    pub plp: Option<u8>,
}

/// The persisted channel catalogue.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChannelDb {
    pub channels: Vec<SavedChannel>,
    #[serde(default)]
    next_id: u64,
}

impl ChannelDb {
    /// Loads the catalogue from disk; returns an empty database if missing.
    pub fn load() -> Self {
        let path = crate::config::channels_file_path();
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(db) = serde_json::from_str::<ChannelDb>(&text) {
                return db;
            }
        }
        Self::default()
    }

    /// Persists the catalogue to disk.
    pub fn save(&self) -> std::io::Result<()> {
        let path = crate::config::channels_file_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(&path, text)
    }

    /// Finds a channel by exact name, else by a unique case-insensitive match.
    pub fn find_by_name(&self, name: &str) -> anyhow::Result<SavedChannel> {
        let needle = name.trim();
        if let Some(ch) = self.channels.iter().find(|c| c.name == needle) {
            return Ok(ch.clone());
        }
        let lower = needle.to_ascii_lowercase();
        let exact: Vec<_> = self
            .channels
            .iter()
            .filter(|c| c.name.to_ascii_lowercase() == lower)
            .collect();
        let matches = if exact.is_empty() {
            self.channels
                .iter()
                .filter(|c| c.name.to_ascii_lowercase().contains(&lower))
                .collect()
        } else {
            exact
        };
        match matches.as_slice() {
            [] => anyhow::bail!("channel not found in channels.json: {needle:?}"),
            [one] => Ok((*one).clone()),
            many => anyhow::bail!("ambiguous channel name {needle:?} ({} matches)", many.len()),
        }
    }

    /// Returns channels for a delivery type, sorted by scan order.
    pub fn for_delivery(&self, delivery: &ChannelDelivery) -> Vec<&SavedChannel> {
        let mut list: Vec<_> = self
            .channels
            .iter()
            .filter(|c| &c.delivery == delivery)
            .collect();
        list.sort_by_key(|c| c.scan_index);
        list
    }

    /// Appends channels, skipping duplicates (same delivery + service_id + frequency).
    pub fn add_batch(&mut self, entries: Vec<SavedChannel>) -> usize {
        let mut added = 0usize;
        for mut ch in entries {
            let dup = self.channels.iter().any(|c| {
                c.delivery == ch.delivery
                    && c.service_id == ch.service_id
                    && c.frequency_mhz == ch.frequency_mhz
                    && c.polarity == ch.polarity
                    && c.satellite == ch.satellite
            });
            if dup {
                continue;
            }
            if self.next_id == 0 {
                self.next_id = self
                    .channels
                    .iter()
                    .map(|c| c.id)
                    .max()
                    .unwrap_or(0)
                    .saturating_add(1);
            }
            ch.id = self.next_id;
            self.next_id += 1;
            self.channels.push(ch);
            added += 1;
        }
        added
    }

    /// Removes a channel by id; returns true if one was removed.
    pub fn remove(&mut self, id: u64) -> bool {
        if let Some(i) = self.channels.iter().position(|c| c.id == id) {
            self.channels.remove(i);
            true
        } else {
            false
        }
    }

    /// All channels sorted by scan order.
    pub fn sorted_all(&self) -> Vec<&SavedChannel> {
        let mut list: Vec<_> = self.channels.iter().collect();
        list.sort_by_key(|c| c.scan_index);
        list
    }

    /// Count of channels per delivery type.
    pub fn count_for(&self, delivery: &ChannelDelivery) -> usize {
        self.channels
            .iter()
            .filter(|c| &c.delivery == delivery)
            .count()
    }
}

impl SavedChannel {
    /// Short delivery tag for list rows: SAT / TER / CAB.
    pub fn delivery_prefix(&self) -> &'static str {
        match self.delivery {
            ChannelDelivery::Satellite => "SAT",
            ChannelDelivery::Terrestrial => "TER",
            ChannelDelivery::Cable => "CAB",
        }
    }

    /// One-line tuning summary shown beside the channel name.
    pub fn tune_summary(&self) -> String {
        match self.delivery {
            ChannelDelivery::Satellite => {
                let pol = self
                    .polarity
                    .as_deref()
                    .and_then(|p| p.chars().next())
                    .map(|c| c.to_ascii_uppercase())
                    .unwrap_or('?');
                let sat = self.satellite.as_deref().map(|s| format!("{s} ")).unwrap_or_default();
                format!(
                    "{sat}{} {} {} {}  sid={}",
                    self.frequency_mhz.unwrap_or(0),
                    pol,
                    self.symbol_rate_ksps.unwrap_or(0),
                    self.delivery_system.as_deref().unwrap_or("DVB-S"),
                    self.service_id
                )
            }
            ChannelDelivery::Terrestrial => {
                let modu = self.modulation.as_deref().unwrap_or("");
                format!(
                    "UHF {} MHz {} {}  sid={}",
                    self.frequency_mhz.unwrap_or(0),
                    self.delivery_system.as_deref().unwrap_or("DVB-T"),
                    modu,
                    self.service_id
                )
            }
            ChannelDelivery::Cable => format!(
                "{} MHz {} {} kSym  sid={}",
                self.frequency_mhz.unwrap_or(0),
                self.delivery_system.as_deref().unwrap_or("DVB-C"),
                self.symbol_rate_ksps.unwrap_or(0),
                self.service_id
            ),
        }
    }

    /// Full list row: `[SAT] Kanal D          618 MHz DVB-T2 …`
    pub fn list_label(&self) -> String {
        // Truncate by characters: names are UTF-8 (Polish, Turkish, …).
        let name = if self.name.chars().count() > 28 {
            format!("{}…", self.name.chars().take(27).collect::<String>())
        } else {
            self.name.clone()
        };
        format!(
            "[{}] {:<28} {}",
            self.delivery_prefix(),
            name,
            self.tune_summary()
        )
    }

    /// Builds capture tuning parameters for live watch.
    pub fn to_capture_params(
        &self,
        device_name: String,
        signal_timeout_s: u32,
    ) -> anyhow::Result<crate::pipeline::CaptureParams> {
        let delivery_system = self
            .delivery_system
            .clone()
            .ok_or_else(|| anyhow::anyhow!("channel '{}' has no delivery_system", self.name))?;
        let frequency_mhz = self
            .frequency_mhz
            .ok_or_else(|| anyhow::anyhow!("channel '{}' has no frequency", self.name))?;
        Ok(crate::pipeline::CaptureParams {
            device_name: Some(device_name),
            adapter: None,
            delivery_system,
            frequency_hz: frequency_mhz as u64 * 1_000_000,
            polarity: self.polarity.clone(),
            symbol_rate: self.symbol_rate_ksps.map(|s| s as u64 * 1_000),
            modulation: self.modulation.clone(),
            fec: self.fec.clone(),
            lnb: match self.delivery {
                ChannelDelivery::Satellite => self.lnb.clone(),
                _ => None,
            },
            satellite_number: self.satellite_number,
            bandwidth_hz: self.bandwidth_mhz.map(|b| b as u64 * 1_000_000),
            guard_interval: self.guard_interval.clone(),
            plp: self.plp,
            dvb_status_interval_s: None,
            signal_timeout_s,
            seconds: 0,
            output: String::new(),
            live_analyze: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_label_truncates_utf8_names_without_panicking() {
        let ch = SavedChannel {
            name: "Telewizja Polska Łódź Śląsk Żółć Kanał HD".into(),
            ..Default::default()
        };
        let label = ch.list_label();
        assert!(label.contains("Łódź"));
        assert!(label.contains('…'));
    }

    #[test]
    fn loads_v1_channel_records() {
        let v1 = r#"{"channels":[{"id":1,"delivery":"satellite","name":"TRT1 HD",
            "service_id":10601,"provider":"TRT","scrambled":false,"scan_index":1,
            "device_name":"/dev/dvb/adapter0","delivery_system":"DVB-S2",
            "frequency_mhz":11054,"polarity":"vertical","symbol_rate_ksps":30000,
            "modulation":"8-PSK","fec":"3/4","satellite_number":0}]}"#;
        let db: ChannelDb = serde_json::from_str(v1).unwrap();
        let ch = &db.channels[0];
        assert_eq!(ch.satellite, None);
        assert_eq!(ch.tuner_model, None);
        assert_eq!(ch.frequency_mhz, Some(11054));
    }

    #[test]
    fn satellite_channel_passes_lnb_to_tsp() {
        let ch = SavedChannel {
            delivery: ChannelDelivery::Satellite,
            name: "TVP INFO HD".into(),
            service_id: 4409,
            delivery_system: Some("DVB-S2".into()),
            frequency_mhz: Some(10719),
            polarity: Some("vertical".into()),
            symbol_rate_ksps: Some(27500),
            lnb: Some("Extended".into()),
            satellite: Some("Hot Bird 13E".into()),
            ..Default::default()
        };
        let args = ch.to_capture_params("/dev/dvb/adapter0:1".into(), 10).unwrap().to_tsp_args();
        let i = args.iter().position(|a| a == "--lnb").unwrap();
        assert_eq!(args[i + 1], "Extended");
        assert!(ch.tune_summary().starts_with("Hot Bird 13E 10719 V"));
    }

    #[test]
    fn same_service_on_two_satellites_is_not_a_duplicate() {
        let mk = |sat: &str| SavedChannel {
            service_id: 1,
            frequency_mhz: Some(11000),
            satellite: Some(sat.into()),
            ..Default::default()
        };
        let mut db = ChannelDb::default();
        assert_eq!(db.add_batch(vec![mk("A"), mk("B"), mk("A")]), 2);
    }
}

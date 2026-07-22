//! Persisted TV channel catalogue (scan results the user chose to keep).
//!
//! Channels are stored in `<data_dir>/channels.json` and listed in scan order.
//! Each entry carries enough tuning metadata to play the service back without
//! re-entering transponder parameters.

use serde::{Deserialize, Serialize};

/// Delivery category for a saved channel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChannelDelivery {
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedChannel {
    pub id: u64,
    pub delivery: ChannelDelivery,
    pub name: String,
    pub service_id: u64,
    pub provider: String,
    pub scrambled: bool,
    /// Order in which the channel was saved (lower = earlier in the list).
    pub scan_index: u32,
    pub device_name: Option<String>,
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

    /// Clears all channels for one delivery type.
    pub fn clear_delivery(&mut self, delivery: &ChannelDelivery) -> usize {
        let before = self.channels.len();
        self.channels.retain(|c| &c.delivery != delivery);
        before - self.channels.len()
    }

    /// All channels sorted by scan order.
    pub fn sorted_all(&self) -> Vec<&SavedChannel> {
        let mut list: Vec<_> = self.channels.iter().collect();
        list.sort_by_key(|c| c.scan_index);
        list
    }

    /// Count of channels per delivery type.
    pub fn count_for(&self, delivery: &ChannelDelivery) -> usize {
        self.channels.iter().filter(|c| &c.delivery == delivery).count()
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
                format!(
                    "{} {} {} {}  sid={}",
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
        let name = if self.name.len() > 28 {
            format!("{}…", &self.name[..27])
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
            lnb: None,
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

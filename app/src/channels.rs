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
}

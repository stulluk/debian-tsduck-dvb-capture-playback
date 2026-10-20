//! Populate [`ChannelDb`] from capture sidecars (`*_services.json`).

use crate::analyze::{CaptureServicesRecord, TuneRecord};
use crate::channels::{ChannelDb, ChannelDelivery, SavedChannel};
use crate::devices::{DeliveryCategory, Discovery};
use crate::lastlog;
use std::path::{Path, PathBuf};

/// Result of a sidecar import pass.
#[derive(Debug, Clone, Default)]
pub struct ImportReport {
    pub sidecars_read: usize,
    pub channels_added: usize,
    pub sidecars_skipped: usize,
}

/// Scans `capture_dir` for `*_services.json` and merges FTA services into `db`.
pub fn import_from_capture_dir(
    capture_dir: &Path,
    db: &mut ChannelDb,
    discovery: Option<&Discovery>,
) -> ImportReport {
    let mut report = ImportReport::default();
    let mut sidecars = Vec::new();
    collect_sidecars(capture_dir, &mut sidecars);
    sidecars.sort();

    let mut batch = Vec::new();
    let mut scan_index = db
        .channels
        .iter()
        .map(|c| c.scan_index)
        .max()
        .unwrap_or(0)
        .saturating_add(1);

    for path in sidecars {
        report.sidecars_read += 1;
        let Ok(record) = CaptureServicesRecord::read_file(&path) else {
            report.sidecars_skipped += 1;
            continue;
        };
        let Some(delivery) = delivery_from_tune(&record.tune) else {
            report.sidecars_skipped += 1;
            continue;
        };
        let device_hint = device_hint_from_path(&path);
        let device_name = discovery.and_then(|d| resolve_device_hint(d, &device_hint, &delivery));

        for svc in record.services.iter().filter(|s| !s.scrambled) {
            batch.push(saved_channel_from_tune(
                &record.tune,
                delivery.clone(),
                &svc.name,
                svc.id,
                &svc.provider,
                scan_index,
                device_name.clone(),
            ));
            scan_index += 1;
        }
    }

    report.channels_added = db.add_batch(batch);
    lastlog::event(
        "channels.import",
        &format!(
            "sidecars={} added={} skipped={} dir={}",
            report.sidecars_read,
            report.channels_added,
            report.sidecars_skipped,
            capture_dir.display()
        ),
    );
    report
}

fn collect_sidecars(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_sidecars(&path, out);
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with("_services.json"))
        {
            out.push(path);
        }
    }
}

fn delivery_from_tune(tune: &TuneRecord) -> Option<ChannelDelivery> {
    let sys = tune.delivery_system.to_ascii_uppercase();
    if sys.starts_with("DVB-S") || sys.starts_with("ISDB-S") {
        Some(ChannelDelivery::Satellite)
    } else if sys.starts_with("DVB-C") {
        Some(ChannelDelivery::Cable)
    } else if sys.starts_with("DVB-T")
        || sys.starts_with("ISDB-T")
        || sys.starts_with("ATSC")
        || sys == "DTMB"
    {
        Some(ChannelDelivery::Terrestrial)
    } else {
        None
    }
}

fn device_hint_from_path(path: &Path) -> Option<String> {
    let s = path.to_string_lossy().to_lowercase();
    if s.contains("pctv461") || s.contains("/pctv") {
        return Some("pctv461".into());
    }
    if s.contains("/terrestrial/") || s.contains("terr_") || s.contains("/618mhz/") {
        return Some("tbs5530-terr".into());
    }
    if s.contains("/cable/") || s.contains("dvbc") || s.contains("vectra") {
        return Some("tbs5530-cable".into());
    }
    if s.contains("tbs5530") {
        return Some("tbs5530".into());
    }
    None
}

fn resolve_device_hint(
    discovery: &Discovery,
    hint: &Option<String>,
    delivery: &ChannelDelivery,
) -> Option<String> {
    let hint = hint.as_deref()?;
    let cat = delivery_category(delivery);
    match hint {
        "pctv461" => discovery
            .devices
            .iter()
            .find(|d| d.label.contains("PCTV") || d.label.contains("461"))
            .map(|d| d.capture_address(DeliveryCategory::Satellite)),
        "tbs5530-terr" => discovery
            .terrestrial_tuner_address()
            .or_else(|| tuner_address(discovery, DeliveryCategory::Terrestrial)),
        "tbs5530-cable" => tuner_address(discovery, DeliveryCategory::Cable),
        "tbs5530" => tuner_address(discovery, cat),
        _ => tuner_address(discovery, cat),
    }
}

fn tuner_address(discovery: &Discovery, cat: DeliveryCategory) -> Option<String> {
    discovery.tuner_for_category(cat).map(|d| d.capture_address(cat))
}

fn delivery_category(delivery: &ChannelDelivery) -> DeliveryCategory {
    match delivery {
        ChannelDelivery::Satellite => DeliveryCategory::Satellite,
        ChannelDelivery::Terrestrial => DeliveryCategory::Terrestrial,
        ChannelDelivery::Cable => DeliveryCategory::Cable,
    }
}

fn saved_channel_from_tune(
    tune: &TuneRecord,
    delivery: ChannelDelivery,
    name: &str,
    service_id: u64,
    provider: &str,
    scan_index: u32,
    device_name: Option<String>,
) -> SavedChannel {
    SavedChannel {
        id: 0,
        delivery,
        name: name.to_string(),
        service_id,
        provider: provider.to_string(),
        scrambled: false,
        scan_index,
        device_name,
        delivery_system: Some(tune.delivery_system.clone()),
        frequency_mhz: Some(tune.frequency_mhz),
        polarity: if tune.polarity.is_empty() {
            None
        } else {
            Some(tune.polarity.clone())
        },
        symbol_rate_ksps: if tune.symbol_rate_ksps > 0 {
            Some(tune.symbol_rate_ksps)
        } else {
            None
        },
        modulation: if tune.modulation.is_empty() {
            None
        } else {
            Some(tune.modulation.clone())
        },
        fec: if tune.fec.is_empty() {
            None
        } else {
            Some(tune.fec.clone())
        },
        satellite_number: tune.satellite_number,
        satellite: tune.satellite.clone(),
        lnb: tune.lnb.clone(),
        tuner_model: tune.tuner_model.clone(),
        bandwidth_mhz: if delivery_from_tune(tune) == Some(ChannelDelivery::Terrestrial) {
            Some(8)
        } else {
            None
        },
        guard_interval: if tune.delivery_system.to_ascii_uppercase().starts_with("DVB-T") {
            Some("19/256".into())
        } else {
            None
        },
        plp: if tune.delivery_system.to_ascii_uppercase().contains("T2") {
            Some(0)
        } else {
            None
        },
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivery_from_dvbt2_tune() {
        let tune = TuneRecord {
            frequency_mhz: 618,
            polarity: String::new(),
            symbol_rate_ksps: 0,
            delivery_system: "DVB-T2".into(),
            modulation: "64-QAM".into(),
            fec: "2/3".into(),
            ..Default::default()
        };
        assert_eq!(delivery_from_tune(&tune), Some(ChannelDelivery::Terrestrial));
    }

    #[test]
    fn path_hint_terrestrial() {
        let p = PathBuf::from("/home/x/dvbhub-captures/tbs5530/terrestrial/618mhz/x_services.json");
        assert_eq!(device_hint_from_path(&p).as_deref(), Some("tbs5530-terr"));
    }
}

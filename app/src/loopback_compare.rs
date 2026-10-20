//! L2a / L2b / L2c comparison of a source TS and its loopback re-capture.
//!
//! A play → modulate → demodulate → capture chain is never byte-identical
//! (random start offset, null-packet stuffing, loop seams), so equality is
//! checked in layers (see `docs/design_consideration.md`):
//!
//! * **L2a link health** — the re-capture has no continuity errors after the
//!   lead-in (first 2 % of packets) beyond what the source itself contains:
//!   an infinite playout repeats the source's own errors once per loop and
//!   adds one discontinuity per PID at every loop seam.
//! * **L2b structure** — same transport-stream id and the same service ids.
//! * **L2c elementary stream** — every video packet payload of the re-capture
//!   (after the lead-in) exists bit-for-bit in the source.

use crate::analyze;
use crate::config::TsduckPaths;
use crate::tsduck;
use serde::Serialize;
use std::collections::HashSet;
use std::io::{BufReader, Read};
use std::path::Path;

const PKT: usize = 188;
/// Share of the re-capture treated as lead-in (lock / tsp start-up).
const LEAD_IN: f64 = 0.02;
/// L2c pass threshold for the payload match ratio after the lead-in.
const L2C_MIN_RATIO: f64 = 0.999;

/// Full comparison result.
#[derive(Debug, Clone, Serialize)]
pub struct CompareReport {
    pub l2a: LinkCheck,
    pub l2b: StructureCheck,
    pub l2c: EsCheck,
}

#[derive(Debug, Clone, Serialize)]
pub struct LinkCheck {
    pub packets: u64,
    pub cc_errors_total: usize,
    pub cc_errors_after_lead_in: usize,
    /// Continuity errors already present in the source file.
    pub source_cc_errors: usize,
    /// Upper bound explained by the source (errors × loops + PIDs × seams).
    pub expected_max: usize,
    pub pass: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct StructureCheck {
    pub source_ts_id: Option<u64>,
    pub capture_ts_id: Option<u64>,
    pub source_services: Vec<u64>,
    pub capture_services: Vec<u64>,
    pub pass: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct EsCheck {
    pub pid: Option<u16>,
    pub source_payloads: usize,
    pub capture_payloads: usize,
    pub matched: usize,
    pub ratio: f64,
    pub pass: bool,
}

impl CompareReport {
    pub fn pass(&self) -> bool {
        self.l2a.pass && self.l2b.pass && self.l2c.pass
    }

    /// Human-readable PASS/FAIL table.
    pub fn print(&self) {
        let v = |b: bool| if b { "PASS" } else { "FAIL" };
        println!(
            "L2a link      {}  {} packets, {} CC errors after lead-in (≤ {} explained by the source, which has {})",
            v(self.l2a.pass),
            self.l2a.packets,
            self.l2a.cc_errors_after_lead_in,
            self.l2a.expected_max,
            self.l2a.source_cc_errors
        );
        println!(
            "L2b structure {}  TS id {:?} vs {:?}, {} vs {} services",
            v(self.l2b.pass),
            self.l2b.source_ts_id,
            self.l2b.capture_ts_id,
            self.l2b.source_services.len(),
            self.l2b.capture_services.len()
        );
        println!(
            "L2c ES        {}  video PID {}: {}/{} re-captured payloads found in source ({:.3} %)",
            v(self.l2c.pass),
            self.l2c.pid.map(|p| format!("0x{p:04X}")).unwrap_or_else(|| "-".into()),
            self.l2c.matched,
            self.l2c.capture_payloads,
            self.l2c.ratio * 100.0
        );
        println!("Overall       {}", v(self.pass()));
    }
}

/// Compares `source` with its loopback `capture`.
pub fn compare(paths: &TsduckPaths, source: &Path, capture: &Path) -> anyhow::Result<CompareReport> {
    let src = analyze::analyze_file(paths, &source.to_string_lossy())?;
    let cap = analyze::analyze_file(paths, &capture.to_string_lossy())?;
    let mut src_ids: Vec<u64> = src.services.iter().map(|s| s.id).collect();
    let mut cap_ids: Vec<u64> = cap.services.iter().map(|s| s.id).collect();
    src_ids.sort_unstable();
    cap_ids.sort_unstable();
    let l2b = StructureCheck {
        pass: src.ts_id.is_some() && src.ts_id == cap.ts_id && src_ids == cap_ids,
        source_ts_id: src.ts_id,
        capture_ts_id: cap.ts_id,
        source_services: src_ids,
        capture_services: cap_ids,
    };

    let packets = std::fs::metadata(capture)?.len() / PKT as u64;
    let lead_in = (packets as f64 * LEAD_IN) as u64;
    let cc = continuity_error_indices(paths, capture)?;
    let after = cc.iter().filter(|&&i| i > lead_in).count();
    let src_cc = continuity_error_indices(paths, source)?.len();
    let src_packets = (std::fs::metadata(source)?.len() / PKT as u64).max(1);
    let (src_pids, _) = pid_census(source)?;
    let loops = packets.div_ceil(src_packets) as usize;
    let seams = loops; // the capture can span one more seam than full loops
    let expected_max = src_cc * loops + src_pids * seams;
    let l2a = LinkCheck {
        packets,
        cc_errors_total: cc.len(),
        cc_errors_after_lead_in: after,
        source_cc_errors: src_cc,
        expected_max,
        pass: packets > 0 && after <= expected_max,
    };

    let pid = video_pid(paths, source)?;
    let l2c = match pid {
        Some(pid) => {
            let src_set: HashSet<u64> = payload_hashes(source, pid, 0)?.into_iter().collect();
            let cap_hashes = payload_hashes(capture, pid, lead_in)?;
            let matched = cap_hashes.iter().filter(|h| src_set.contains(h)).count();
            let ratio = if cap_hashes.is_empty() {
                0.0
            } else {
                matched as f64 / cap_hashes.len() as f64
            };
            EsCheck {
                pid: Some(pid),
                source_payloads: src_set.len(),
                capture_payloads: cap_hashes.len(),
                matched,
                ratio,
                pass: !cap_hashes.is_empty() && ratio >= L2C_MIN_RATIO,
            }
        }
        None => EsCheck {
            pid: None,
            source_payloads: 0,
            capture_payloads: 0,
            matched: 0,
            ratio: 0.0,
            pass: false,
        },
    };
    Ok(CompareReport { l2a, l2b, l2c })
}

/// Packet indices where `tsp -P continuity` reports a discontinuity.
fn continuity_error_indices(paths: &TsduckPaths, file: &Path) -> anyhow::Result<Vec<u64>> {
    let out = tsduck::tool_command(paths, "tsp")
        .args(["-I", "file"])
        .arg(file)
        .args(["-P", "continuity", "-O", "drop"])
        .output()?;
    let text = String::from_utf8_lossy(&out.stderr);
    // Lines look like "* continuity: packet index: 1,234, PID: 0x00AC (172), missing 3 packets".
    Ok(text
        .lines()
        .filter_map(|l| l.split("packet index: ").nth(1)?.split(", PID").next())
        .filter_map(|n| n.chars().filter(char::is_ascii_digit).collect::<String>().parse().ok())
        .collect())
}

/// Number of distinct PIDs (excluding null packets) and packet count of a file.
fn pid_census(file: &Path) -> anyhow::Result<(usize, u64)> {
    let mut r = BufReader::with_capacity(1 << 20, std::fs::File::open(file)?);
    let mut buf = [0u8; PKT];
    let mut seen = vec![false; 0x2000];
    let mut packets = 0u64;
    while r.read_exact(&mut buf).is_ok() {
        packets += 1;
        if buf[0] == 0x47 {
            seen[((usize::from(buf[1] & 0x1F)) << 8) | usize::from(buf[2])] = true;
        }
    }
    seen[0x1FFF] = false;
    Ok((seen.iter().filter(|s| **s).count(), packets))
}

/// Busiest video PID of a file (from `tsp -P analyze --json`, first 200k packets).
fn video_pid(paths: &TsduckPaths, file: &Path) -> anyhow::Result<Option<u16>> {
    let args: Vec<String> = [
        "-I", "file", &file.to_string_lossy(), "-P", "until", "--packets", "200000",
        "-P", "analyze", "--json", "-O", "drop",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let v = tsduck::run_json(paths, "tsp", &args.iter().map(String::as_str).collect::<Vec<_>>())?;
    let best = v
        .get("pids")
        .and_then(|p| p.as_array())
        .into_iter()
        .flatten()
        .filter(|p| {
            p.get("description")
                .and_then(|d| d.as_str())
                .is_some_and(|d| d.to_ascii_lowercase().contains("video"))
        })
        .max_by_key(|p| p.get("packets").and_then(|n| n.get("total")).and_then(|t| t.as_u64()).unwrap_or(0))
        .and_then(|p| p.get("id").and_then(|i| i.as_u64()))
        .and_then(|i| u16::try_from(i).ok());
    Ok(best)
}

/// FNV-1a hashes of the payloads of `pid` packets, skipping the first `skip` packets.
pub fn payload_hashes(file: &Path, pid: u16, skip: u64) -> anyhow::Result<Vec<u64>> {
    let mut r = BufReader::with_capacity(1 << 20, std::fs::File::open(file)?);
    let mut buf = [0u8; PKT];
    let mut out = Vec::new();
    let mut index = 0u64;
    while r.read_exact(&mut buf).is_ok() {
        index += 1;
        if index <= skip || buf[0] != 0x47 {
            continue;
        }
        if let Some(payload) = packet_payload(&buf, pid) {
            out.push(fnv1a(payload));
        }
    }
    Ok(out)
}

/// Payload bytes of a packet on `pid` (None for other PIDs or no payload).
fn packet_payload(pkt: &[u8; PKT], pid: u16) -> Option<&[u8]> {
    let p = (u16::from(pkt[1] & 0x1F) << 8) | u16::from(pkt[2]);
    if p != pid {
        return None;
    }
    let afc = (pkt[3] >> 4) & 0x3;
    let start = match afc {
        0b01 => 4,
        0b11 => 5 + pkt[4] as usize,
        _ => return None,
    };
    (start < PKT).then(|| &pkt[start..])
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01B3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkt(pid: u16, afc: u8, fill: u8) -> [u8; PKT] {
        let mut p = [fill; PKT];
        p[0] = 0x47;
        p[1] = ((pid >> 8) & 0x1F) as u8;
        p[2] = (pid & 0xFF) as u8;
        p[3] = afc << 4;
        if afc == 0b11 {
            p[4] = 7; // adaptation field length
        }
        p
    }

    #[test]
    fn payload_offsets() {
        assert_eq!(packet_payload(&pkt(0x100, 0b01, 1), 0x100).unwrap().len(), 184);
        assert_eq!(packet_payload(&pkt(0x100, 0b11, 1), 0x100).unwrap().len(), 176);
        assert!(packet_payload(&pkt(0x100, 0b10, 1), 0x100).is_none());
        assert!(packet_payload(&pkt(0x101, 0b01, 1), 0x100).is_none());
    }

    #[test]
    fn hashes_match_regardless_of_continuity_counter() {
        let mut a = pkt(0x100, 0b01, 9);
        let mut b = a;
        a[3] |= 0x3;
        b[3] |= 0xC; // different CC, same payload
        assert_eq!(fnv1a(packet_payload(&a, 0x100).unwrap()), fnv1a(packet_payload(&b, 0x100).unwrap()));
    }
}

//! MPEG-TS Program Clock Reference (PCR) extraction from 188-byte packets.
//!
//! Broadcasters embed PCR in the adaptation field (ISO/IEC 13818-1). Players like
//! Kaffeine (Xine) and Kodi (FFmpeg demuxer) lock playback to PCR or running time;
//! GStreamer `tsdemux` can either honour PCR (`ignore-pcr=false`) or stamp buffers
//! with local running time (`ignore-pcr=true`) for live sources without a PCR clock.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// PCR state shared between the TS feeder and stats thread.
#[derive(Clone, Default)]
pub struct PcrState {
    /// First PCR seen (27 MHz ticks), for relative timing.
    pub origin_pcr: Arc<Mutex<Option<u64>>>,
    pub last_pcr: Arc<AtomicU64>,
    pub pcr_count: Arc<AtomicU64>,
    /// Wall clock when the first decoded video buffer was observed.
    pub playback_wall_start: Arc<Mutex<Option<Instant>>>,
}

impl PcrState {
    /// Records a PCR sample; sets origin on the first value.
    pub fn observe_pcr(&self, pcr_27mhz: u64) {
        self.pcr_count.fetch_add(1, Ordering::Relaxed);
        self.last_pcr.store(pcr_27mhz, Ordering::Relaxed);
        // Origin is set when the first video frame arrives (mark_playback_start).
    }

    /// PCR elapsed seconds since the first PCR in the session.
    pub fn pcr_elapsed_secs(&self) -> Option<f64> {
        let origin = *self.origin_pcr.lock().ok()?;
        let origin = origin?;
        let last = self.last_pcr.load(Ordering::Relaxed);
        if last <= origin {
            return Some(0.0);
        }
        Some(pcr27mhz_to_secs(last - origin))
    }

    /// Latest absolute PCR as seconds (27 MHz domain).
    pub fn last_pcr_secs(&self) -> Option<f64> {
        let last = self.last_pcr.load(Ordering::Relaxed);
        if last == 0 {
            return None;
        }
        Some(pcr27mhz_to_secs(last))
    }

    pub fn mark_playback_start(&self) {
        if let Ok(mut start) = self.playback_wall_start.lock() {
            if start.is_some() {
                return;
            }
            *start = Some(Instant::now());
            // Align PCR baseline with first decoded video frame (not first TS packet).
            if let Ok(mut origin) = self.origin_pcr.lock() {
                let last = self.last_pcr.load(Ordering::Relaxed);
                if last > 0 {
                    *origin = Some(last);
                }
            }
        }
    }

    pub fn wall_elapsed_secs(&self) -> Option<f64> {
        let start = *self.playback_wall_start.lock().ok()?;
        start.map(|t| t.elapsed().as_secs_f64())
    }
}

/// Parses PCR from a 188-byte MPEG-TS packet. Returns 27 MHz tick count.
pub fn pcr_from_packet(pkt: &[u8]) -> Option<u64> {
    if pkt.len() < 12 || pkt[0] != 0x47 {
        return None;
    }
    let afc = (pkt[3] >> 4) & 0x03;
    if afc != 2 && afc != 3 {
        return None;
    }
    let adapt_len = pkt[4] as usize;
    if adapt_len < 1 || pkt.len() < 6 + adapt_len {
        return None;
    }
    let flags = pkt[5];
    if flags & 0x10 == 0 {
        return None;
    }
    if pkt.len() < 12 {
        return None;
    }
    Some(parse_pcr6(&pkt[6..12]))
}

/// Scans a TS byte chunk (any length) for PCR values in 188-byte packets.
pub fn scan_chunk_for_pcr(data: &[u8], state: &PcrState) {
    for pkt in data.chunks_exact(188) {
        if let Some(pcr) = pcr_from_packet(pkt) {
            state.observe_pcr(pcr);
        }
    }
}

fn parse_pcr6(data: &[u8]) -> u64 {
    let pcr_base = ((data[0] as u64) << 25)
        | ((data[1] as u64) << 17)
        | ((data[2] as u64) << 9)
        | ((data[3] as u64) << 1)
        | ((data[4] as u64) >> 7);
    let pcr_ext = (((data[4] & 0x01) as u64) << 8) | (data[5] as u64);
    pcr_base * 300 + pcr_ext
}

fn pcr27mhz_to_secs(ticks: u64) -> f64 {
    ticks as f64 / 27_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcr_known_vector() {
        // Adaptation: len=7, flags=0x50 (PCR present), PCR bytes crafted.
        let mut pkt = [0u8; 188];
        pkt[0] = 0x47;
        pkt[1] = 0x00;
        pkt[2] = 0x10;
        pkt[3] = 0x30; // adaptation + payload
        pkt[4] = 0x07; // adaptation length
        pkt[5] = 0x50; // PCR flag
                       // PCR base is 33 bits: bytes 6..9 hold bits 32..1, the top bit of
                       // byte 10 holds bit 0. Base = 1 @ 90 kHz, ext = 0 → 300 ticks @ 27 MHz.
        pkt[6] = 0x00;
        pkt[7] = 0x00;
        pkt[8] = 0x00;
        pkt[9] = 0x00;
        pkt[10] = 0x80;
        pkt[11] = 0x00;
        assert_eq!(pcr_from_packet(&pkt), Some(300));
        // Base = 1, ext = 0x102 (9-bit extension spans byte 10 bit 0 + byte 11).
        pkt[10] = 0x81;
        pkt[11] = 0x02;
        assert_eq!(pcr_from_packet(&pkt), Some(300 + 0x102));
    }

    #[test]
    fn pcr_elapsed_tracks_delta() {
        let state = PcrState::default();
        state.observe_pcr(0);
        state.observe_pcr(27_000_000);
        state.mark_playback_start(); // origin = 27_000_000
        state.observe_pcr(54_000_000); // +1s since origin
        let elapsed = state.pcr_elapsed_secs().unwrap();
        assert!((elapsed - 1.0).abs() < 0.001);
    }
}

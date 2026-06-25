//! Reference transponders for Turksat 42E (see docs/reference_transponders.md).
//!
//! These are the user's known-good DVB-S/S2 transponders in Istanbul, used as
//! capture sources and loopback references. Tuning helper values (LNB local
//! oscillator, 22 kHz tone, L-band IF) are derived from a universal Ku-band LNB.

use serde::Serialize;

/// Universal Ku-band LNB low local-oscillator frequency (MHz).
pub const LNB_LO_LOW_MHZ: u32 = 9750;
/// Universal Ku-band LNB high local-oscillator frequency (MHz).
pub const LNB_LO_HIGH_MHZ: u32 = 10600;
/// Ku-band split frequency (MHz): at/above this the high band + 22 kHz tone.
pub const KU_BAND_SPLIT_MHZ: u32 = 11700;

/// Signal polarity.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Polarity {
    Horizontal,
    Vertical,
}

impl Polarity {
    /// TSDuck `--polarity` argument value (used by capture/playback builders).
    #[allow(dead_code)]
    pub fn tsduck(self) -> &'static str {
        match self {
            Polarity::Horizontal => "horizontal",
            Polarity::Vertical => "vertical",
        }
    }

    /// Single-letter form for compact tables ("H" / "V").
    pub fn short(self) -> &'static str {
        match self {
            Polarity::Horizontal => "H",
            Polarity::Vertical => "V",
        }
    }
}

/// A reference transponder definition with the parameters needed to tune it.
#[derive(Debug, Clone, Serialize)]
pub struct Transponder {
    /// 1-based reference index used in the UI/CLI.
    pub id: u32,
    /// Downlink frequency in MHz.
    pub frequency_mhz: u32,
    /// Polarity.
    pub polarity: Polarity,
    /// Symbol rate in kSymbols/s.
    pub symbol_rate_ksps: u32,
    /// Modulation scheme (TSDuck `--modulation` value).
    pub modulation: &'static str,
    /// Inner FEC code rate (TSDuck `--fec-inner` value).
    pub fec: &'static str,
    /// Delivery system (TSDuck `--delivery-system` value).
    pub delivery_system: &'static str,
    /// Short content description.
    pub content: &'static str,
}

impl Transponder {
    /// Returns true if the transponder is in the Ku high band (22 kHz tone on).
    pub fn is_high_band(&self) -> bool {
        self.frequency_mhz >= KU_BAND_SPLIT_MHZ
    }

    /// LNB local oscillator frequency (MHz) for this transponder.
    pub fn lnb_lo_mhz(&self) -> u32 {
        if self.is_high_band() {
            LNB_LO_HIGH_MHZ
        } else {
            LNB_LO_LOW_MHZ
        }
    }

    /// Intermediate (L-band) frequency in MHz presented to the tuner.
    pub fn l_band_if_mhz(&self) -> u32 {
        self.frequency_mhz - self.lnb_lo_mhz()
    }
}

/// The fixed reference transponder set (Turksat 42E).
pub const REFERENCE_TRANSPONDERS: &[Transponder] = &[
    Transponder {
        id: 1,
        frequency_mhz: 11054,
        polarity: Polarity::Vertical,
        symbol_rate_ksps: 30000,
        modulation: "8-PSK",
        fec: "3/4",
        delivery_system: "DVB-S2",
        content: "TRT group (HD)",
    },
    Transponder {
        id: 2,
        frequency_mhz: 11096,
        polarity: Polarity::Horizontal,
        symbol_rate_ksps: 30000,
        modulation: "QPSK",
        fec: "5/6",
        delivery_system: "DVB-S",
        content: "TRT group (SD)",
    },
    Transponder {
        id: 3,
        frequency_mhz: 12245,
        polarity: Polarity::Horizontal,
        symbol_rate_ksps: 27500,
        modulation: "8-PSK",
        fec: "5/6",
        delivery_system: "DVB-S2",
        content: "Dogan Medya (Kanal D HD, CNN Turk)",
    },
    Transponder {
        id: 4,
        frequency_mhz: 12380,
        polarity: Polarity::Vertical,
        symbol_rate_ksps: 27500,
        modulation: "QPSK",
        fec: "3/4",
        delivery_system: "DVB-S",
        content: "Turksat group (TGRT Haber HD); carries NIT",
    },
];

/// Looks up a reference transponder by its 1-based id.
///
/// Used by the capture/playback command builders (added in a later iteration).
#[allow(dead_code)]
pub fn by_id(id: u32) -> Option<&'static Transponder> {
    REFERENCE_TRANSPONDERS.iter().find(|t| t.id == id)
}

//! TSDuck `--lnb` presets (names from `tsduck.lnbs.xml` + common legacy MHz values).

/// One entry in the manual-capture LNB picker.
pub struct LnbPreset {
    /// User-visible label in the TUI.
    pub label: &'static str,
    /// Value passed to `tsp -I dvb --lnb …`.
    pub tsp_value: &'static str,
}

/// Known LNB types supported by TSDuck (`tsduck.lnbs.xml` table + legacy MHz strings).
pub const LNB_PRESETS: &[LnbPreset] = &[
    LnbPreset {
        label: "Universal Ku (9750/10600, 10.7-12.75 GHz)",
        tsp_value: "Extended",
    },
    LnbPreset {
        label: "Universal Ku old (10.8-12.7 GHz)",
        tsp_value: "Universal",
    },
    LnbPreset {
        label: "Single LO 9750 (Astra / Enhanced, 10.7-11.7 GHz)",
        tsp_value: "Astra",
    },
    LnbPreset {
        label: "Universal Ku MHz (9750,10600,11700)",
        tsp_value: "9750,10600,11700",
    },
    LnbPreset {
        label: "Direct L-band injection (--lnb 0)",
        tsp_value: "0",
    },
    LnbPreset {
        label: "C-Band (Big Dish Monopoint)",
        tsp_value: "C-Band",
    },
    LnbPreset {
        label: "C-Band multipoint (C-Multi)",
        tsp_value: "C-Multi",
    },
    LnbPreset {
        label: "C-Band single LO 5150 MHz",
        tsp_value: "5150",
    },
    LnbPreset {
        label: "C-Band single LO 5750 MHz",
        tsp_value: "5750",
    },
    LnbPreset {
        label: "C-Band single LO 5950 MHz",
        tsp_value: "5950",
    },
    LnbPreset {
        label: "DBS / Expressvu",
        tsp_value: "DBS",
    },
    LnbPreset {
        label: "DBS Standard",
        tsp_value: "Standard",
    },
    LnbPreset {
        label: "DBS LO 10700 MHz",
        tsp_value: "L10700",
    },
    LnbPreset {
        label: "DBS LO 10750 MHz",
        tsp_value: "L10750",
    },
    LnbPreset {
        label: "DBS LO 11300 MHz",
        tsp_value: "L11300",
    },
    LnbPreset {
        label: "DBS single LO 11250 MHz",
        tsp_value: "11250",
    },
    LnbPreset {
        label: "DishPro",
        tsp_value: "DishPro",
    },
    LnbPreset {
        label: "Invacom QPH-031 (circular)",
        tsp_value: "QPH031",
    },
    LnbPreset {
        label: "Stacked BrasilSat",
        tsp_value: "Stacked-Brasilsat",
    },
    LnbPreset {
        label: "BrasilSat Oi",
        tsp_value: "Oi-BrasilSat",
    },
    LnbPreset {
        label: "BrasilSat Amazonas (3 oscillators)",
        tsp_value: "Amazonas3",
    },
    LnbPreset {
        label: "BrasilSat Amazonas (2 oscillators)",
        tsp_value: "Amazonas2",
    },
    LnbPreset {
        label: "BrasilSat GVT",
        tsp_value: "GVT-BrasilSat",
    },
    LnbPreset {
        label: "Japan BS/CS (110BS/CS)",
        tsp_value: "Japan",
    },
    LnbPreset {
        label: "No LNB offset (None)",
        tsp_value: "None",
    },
];

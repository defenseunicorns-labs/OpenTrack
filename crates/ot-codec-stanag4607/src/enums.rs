//! Enumeration (E8) and flag (FL) name tables from Edition 3. Each function
//! cites the table it implements; unknown/reserved codes get a descriptive
//! fallback name rather than an error.
//!
//! Source: STANAG 4607 Ed. 3 (AEDP-7, 2013), from the NSG Standards
//! Registry, https://nsgreg.nga.mil/ (see this crate's README, "Source").

use serde::Serialize;

/// An enumerated value: the code as transmitted plus a readable name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Enumeration {
    pub code: u8,
    pub name: &'static str,
}

impl Enumeration {
    pub(crate) fn new(code: u8, f: fn(u8) -> &'static str) -> Self {
        Self {
            code,
            name: f(code),
        }
    }
}

/// Table 2-1.1, packet security classification.
pub fn classification_name(v: u8) -> &'static str {
    match v {
        1 => "TOP SECRET",
        2 => "SECRET",
        3 => "CONFIDENTIAL",
        4 => "RESTRICTED",
        5 => "UNCLASSIFIED",
        _ => "Reserved",
    }
}

/// Table 2-1.3, packet security codes (bit flags). Returns the names of the set bits.
pub fn security_code_names(v: u16) -> Vec<&'static str> {
    const NAMES: [&str; 16] = [
        "NOCONTRACT",
        "ORCON",
        "PROPIN",
        "WNINTEL",
        "NATIONAL ONLY",
        "LIMDIS",
        "FOUO",
        "EFTO",
        "LIM OFF USE (UNCLAS)",
        "NONCOMPARTMENT",
        "SPECIAL CONTROL",
        "SPECIAL INTEL",
        "WARNING NOTICE - SECURITY CLASSIFICATION IS BASED ON THE FACT OF EXISTENCE AND AVAIL OF THIS DATA",
        "REL NATO",
        "REL 4-EYES",
        "REL 9-EYES",
    ];
    flag_names16(v, &NAMES)
}

/// Table 2-1.4, exercise indicator.
pub fn exercise_indicator_name(v: u8) -> &'static str {
    match v {
        0 => "Operation, Real Data",
        1 => "Operation, Simulated Data",
        2 => "Operation, Synthesized Data",
        128 => "Exercise, Real Data",
        129 => "Exercise, Simulated Data",
        130 => "Exercise, Synthesized Data",
        _ => "Reserved",
    }
}

/// Table 2-2, segment types: the snake_case record name used in output.
pub fn segment_type_name(v: u8) -> &'static str {
    match v {
        1 => "mission",
        2 => "dwell",
        3 => "hrr",
        4 => "range_doppler",
        5 => "job_definition",
        6 => "free_text",
        7 => "low_reflectivity_index",
        8 => "group",
        9 => "attached_target",
        10 => "test_and_status",
        11 => "system_specific",
        12 => "processing_history",
        13 => "platform_location",
        101 => "job_request",
        102 => "job_acknowledge",
        128..=255 => "extension",
        _ => "reserved",
    }
}

/// Table 2-3.1, platform types.
pub fn platform_type_name(v: u8) -> &'static str {
    match v {
        0 => "Unidentified",
        1 => "ACS",
        2 => "ARL-M",
        3 => "Sentinel (was ASTOR)",
        4 => "Rotary Wing Radar (was CRESO)",
        5 => "Global Hawk-Navy",
        6 => "HORIZON",
        7 => "E-8 (Joint STARS)",
        8 => "P-3C",
        9 => "Predator",
        10 => "RADARSAT2",
        11 => "U-2",
        12 => "E-10 (was MC2A)",
        13 => "UGS - Single",
        14 => "UGS - Cluster",
        15 => "Ground Based",
        16 => "UAV-Army",
        17 => "UAV-Marines",
        18 => "UAV-Navy",
        19 => "UAV-Air Force",
        20 => "Global Hawk-Air Force",
        21 => "Global Hawk-Australia",
        22 => "Global Hawk-Germany",
        23 => "Paul Revere",
        24 => "Mariner UAV",
        25 => "BAC-111",
        26 => "Coyote",
        27 => "King Air",
        28 => "LIMIT",
        29 => "NRL NP-3B",
        30 => "SOSTAR-X",
        31 => "WatchKeeper",
        32 => "Alliance Ground Surveillance (AGS) (A321)",
        33 => "Stryker",
        34 => "AGS (HALE UAV)",
        35 => "SIDM",
        36 => "Reaper",
        37 => "Warrior A",
        38 => "Warrior",
        39 => "Twin Otter",
        255 => "Other",
        _ => "Available for Future Use",
    }
}

/// Table 2-4.2, target classification.
///
/// The table lists "143 Tagging Device" followed by "143-253 Reserved"; 142 is
/// otherwise unassigned, so both 142 and 143 are named "Tagging Device".
pub fn target_classification_name(v: u8) -> &'static str {
    match v {
        0 => "No Information, Live Target",
        1 => "Tracked Vehicle, Live Target",
        2 => "Wheeled Vehicle, Live Target",
        3 => "Rotary Wing Aircraft, Live Target",
        4 => "Fixed Wing Aircraft, Live Target",
        5 => "Stationary Rotator, Live Target",
        6 => "Maritime, Live Target",
        7 => "Beacon, Live Target",
        8 => "Amphibious, Live Target",
        9 => "Person, Live Target",
        10 => "Vehicle, Live Target",
        11 => "Animal, Live Target",
        12 => "Large Multiple-Return, Live Land Target",
        13 => "Large Multiple-Return, Live Maritime Target",
        126 => "Other, Live Target",
        127 => "Unknown, Live Target",
        128 => "No Information, Simulated Target",
        129 => "Tracked Vehicle, Simulated Target",
        130 => "Wheeled Vehicle, Simulated Target",
        131 => "Rotary Wing Aircraft, Simulated Target",
        132 => "Fixed Wing Aircraft, Simulated Target",
        133 => "Stationary Rotator, Simulated Target",
        134 => "Maritime, Simulated Target",
        135 => "Beacon, Simulated Target",
        136 => "Amphibious, Simulated Target",
        137 => "Person, Simulated Target",
        138 => "Vehicle, Simulated Target",
        139 => "Animal, Simulated Target",
        140 => "Large Multiple-Return, Simulated Land Target",
        141 => "Large Multiple-Return, Simulated Maritime Target",
        142 | 143 => "Tagging Device",
        254 => "Other, Simulated Target",
        255 => "Unknown, Simulated Target",
        _ => "Reserved",
    }
}

/// HRR H16, compression flag (Table 2-5).
pub fn compression_name(v: u8) -> &'static str {
    match v {
        0 => "No Compression",
        1 => "Threshold Decomposition (x10)",
        _ => "Reserved",
    }
}

/// HRR H17/H18, weighting function type (Table 2-5).
pub fn weighting_name(v: u8) -> &'static str {
    match v {
        0 => "No Statement",
        1 => "Taylor Weighting",
        2 => "Other",
        _ => "Reserved",
    }
}

/// HRR H23, type of HRR/RDM (Table 2-5, para 2.5.23).
pub fn hrr_type_name(v: u8) -> &'static str {
    match v {
        0 => "Other",
        1 => "1-D HRR Chip",
        2 => "2-D HRR Chip",
        3 => "Sparse HRR Chip",
        4 => "Oversized HRR Chip",
        5 => "Full RDM",
        6 => "Partial RDM",
        7 => "Full Range-Pulse Data",
        _ => "Reserved",
    }
}

/// Table 2-7.1, sensor types.
pub fn sensor_type_name(v: u8) -> &'static str {
    match v {
        0 => "Unidentified",
        1 => "Other",
        2 => "HiSAR",
        3 => "ASTOR",
        4 => "Rotary Wing Radar (was CRESO)",
        5 => "Global Hawk Sensor",
        6 => "HORIZON",
        7 => "APY-3",
        8 => "APY-6",
        9 => "APY-8 (Lynx I)",
        10 => "RADARSAT2",
        11 => "ASARS-2A",
        12 => "TESAR",
        13 => "MP-RTIP",
        14 => "APG-77",
        15 => "APG-79",
        16 => "APG-81",
        17 => "APY-6v1",
        18 => "DPY-1 (Lynx II)",
        19 => "SIDM",
        20 => "LIMIT",
        21 => "TCAR (AGS A321)",
        22 => "LSRS Sensor",
        23 => "UGS Single Sensor",
        24 => "UGS Cluster Sensor",
        25 => "IMASTER GMTI",
        26 => "AN/ZPY-1 (STARLite)",
        27 => "VADER",
        255 => "No Statement",
        _ => "Available for Future Use",
    }
}

/// Table 2-7.2, radar modes.
pub fn radar_mode_name(v: u8) -> &'static str {
    match v {
        0 => "Unspecified Mode",
        1 => "MTI (Moving Target Indicator)",
        2 => "HRR (High Range Resolution)",
        3 => "UHRR (Ultra High Range Resolution)",
        4 => "HUR (High Update Rate)",
        5 => "FTI",
        11 => "Attack Control - SATC (Joint STARS)",
        12 => "Attack Control (Joint STARS)",
        13 => "SATC (Joint STARS)",
        14 => "Attack Planning - SATC (Joint STARS)",
        15 => "Attack Planning (Joint STARS)",
        16 => "Medium Resolution Sector Search (Joint STARS)",
        17 => "Low Resolution Sector Search (Joint STARS)",
        18 => "Wide Area Search - GRCA (Joint STARS)",
        19 => "Wide Area Search - RRCA (Joint STARS)",
        20 => "Attack Planning - With Tracking (Joint STARS)",
        21 => "Attack Control - With Tracking (Joint STARS)",
        31 => "Wide Area MTI (WAMTI) (ASARS-AIP)",
        32 => "Coarse Resolution Search (ASARS-AIP)",
        33 => "Medium Resolution Search (ASARS-AIP)",
        34 => "High Resolution Search (ASARS-AIP)",
        35 => "Point Imaging (ASARS-AIP)",
        36 => "Swath MTI (SMTI) (ASARS-AIP)",
        37 => "Repetitive Point Imaging (ASARS-AIP)",
        38 => "Monopulse Calibration (ASARS-AIP)",
        51 => "Search (ASARS-2)",
        52 => "EMTI Wide Frame Search (ASARS-2)",
        53 => "EMTI Narrow Frame Search (ASARS-2)",
        54 => "EMTI Augmented Spot (ASARS-2)",
        55 => "EMTI Wide Area MTI (WAMTI) (ASARS-2)",
        61 => "GMTI PPI Mode (TUAV)",
        62 => "GMTI Expanded Mode (TUAV)",
        63 => "Narrow Sector Search (NSS) (ARL-M)",
        64 => "Single Beam Scan (SBS) (ARL-M)",
        65 => "Wide Area (WA) (ARL-M)",
        81 => "GRCA",
        82 => "RRCA",
        83 => "Sector Search",
        84 => "HORIZON Basic (HORIZON)",
        85 => "HORIZON High Sensitivity (HORIZON)",
        86 => "HORIZON Burn Through (HORIZON)",
        87 => "CRESO Acquisition (CRESO)",
        88 => "CRESO Count (CRESO)",
        94 => "WAS MTI EXO (ASTOR)",
        95 => "WAS MTI ENDO/EXO (ASTOR)",
        96 => "SS MTI EXO (ASTOR)",
        97 => "SS MTI ENDO/EXO (ASTOR)",
        100 => "Test/Status Mode",
        101 => "MTI Spot Scan (Lynx I/II)",
        102 => "MTI Arc Scan (Lynx I/II)",
        103 => "HRR/MTI Spot Scan (Lynx I/II)",
        104 => "HRR/MTI Arc Scan (Lynx I/II)",
        111 => "GRCA (Global Hawk)",
        112 => "RRCA (Global Hawk)",
        113 => "GMTI-HRR (Global Hawk)",
        120 => "Small-Area GMTI (VADER)",
        121 => "Wide-Area GMTI (VADER)",
        122 => "Dismount GMTI (VADER)",
        123 => "HRR GMTI (VADER)",
        _ => "Available for Future Use",
    }
}

/// Table 2-7.3, terrain elevation models.
pub fn terrain_model_name(v: u8) -> &'static str {
    match v {
        0 => "None Specified",
        1 => "DTED0",
        2 => "DTED1",
        3 => "DTED2",
        4 => "DTED3",
        5 => "DTED4",
        6 => "DTED5",
        7 => "SRTM1",
        8 => "SRTM2",
        9 => "DGM50 M745",
        10 => "DGM250",
        11 => "ITHD",
        12 => "STHD",
        13 => "SEDRIS",
        _ => "Reserved",
    }
}

/// Table 2-7.4, geoid models.
pub fn geoid_model_name(v: u8) -> &'static str {
    match v {
        0 => "None Specified",
        1 => "EGM96",
        2 => "GEO96",
        3 => "Flat Earth",
        _ => "Reserved",
    }
}

/// Table 2-14.2, processing performed (bit flags).
pub fn processing_performed_names(v: u16) -> Vec<&'static str> {
    const NAMES: [&str; 16] = [
        "Area Filtering",
        "Target Classification Filtering",
        "LOS Velocity Filtering",
        "SNR Filtering",
        "De-clutter Filtering",
        "Bandwidth Filtering",
        "Revisit Filtering",
        "Location Adjustment",
        "Geoid Adjustment",
        "Location Registration",
        "Time Filtering",
        "Security Filtering",
        "Data Augmentation",
        "Target Coordinate Conversion",
        "Reserved (0x4000)",
        "Reserved (0x8000)",
    ];
    flag_names16(v, &NAMES)
}

/// Table 3-2 field A18, job request status.
pub fn request_status_name(v: u8) -> &'static str {
    match v {
        0 => "Request",
        1 => "Approved",
        2 => "Approved, with Modification",
        3 => "Denied: Line of Sight",
        4 => "Denied: Timeline",
        5 => "Denied: Orbit",
        6 => "Denied: Priority",
        7 => "Denied: Area of Interest",
        8 => "Denied: Illegal Request",
        9 => "Denied: Function Inoperative",
        10 => "Denied: Other",
        _ => "Reserved",
    }
}

fn flag_names16(v: u16, names: &[&'static str; 16]) -> Vec<&'static str> {
    names
        .iter()
        .enumerate()
        .filter(|(i, _)| v & (1u16 << i) != 0)
        .map(|(_, n)| *n)
        .collect()
}

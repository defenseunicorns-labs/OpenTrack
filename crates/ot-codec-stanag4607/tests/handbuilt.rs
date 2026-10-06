//! Hand-built packets for each segment type, built from the Annex A layouts,
//! exercising existence-mask combinations and unit conversions.

mod common;

use common::{B, job_definition_body, mission_body, packet, segment};
use ot_codec_stanag4607::{
    Decoder, Emit, Error, Options, Segment, VERSION, packet_len, parse_packet,
};

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}
fn sa32(raw: i32) -> f64 {
    f64::from(raw) * 180.0 / 4_294_967_296.0
}
fn ba32(raw: u32) -> f64 {
    f64::from(raw) * 360.0 / 4_294_967_296.0
}

const SENSOR_LAT_RAW: i32 = -823_386_634; // ~ -34.5 deg
const CENTER_LAT_RAW: i32 = -811_271_086; // ~ -34.0 deg
const CENTER_LON_RAW: u32 = 4_181_922_734; // ~ 350.53 deg
const LAT_SCALE_RAW: i32 = 1000;
const LON_SCALE_RAW: u32 = 2000;

/// Dwell with every dwell field and the reduced-bandwidth (delta) target form
/// plus all optional target fields. Mask: FF FF FF FE 7F FF 00 00.
fn dwell_delta_body() -> Vec<u8> {
    let mut b = B::new()
        .bytes(&[0xFF, 0xFF, 0xFF, 0xFE, 0x7F, 0xFF, 0x00, 0x00])
        .u16(3) // D2
        .u16(4) // D3
        .u8(1) // D4
        .u16(2) // D5
        .u32(3_600_500) // D6 01:00:00.500
        .i32(SENSOR_LAT_RAW) // D7
        .ba32(350.0) // D8
        .i32(1_000_000) // D9 cm
        .i32(LAT_SCALE_RAW) // D10
        .u32(LON_SCALE_RAW) // D11
        .u32(150) // D12
        .u32(250) // D13
        .u16(300) // D14
        .u16(0x4000) // D15 BA16 90
        .u32(200_000) // D16 mm/s
        .i8(-5) // D17 dm/s
        .u8(2) // D18
        .u16(1500) // D19
        .u16(20) // D20
        .u16(0x591C) // D21 BA16 = 125.31006 (Annex B example)
        .i16(-12698) // D22 SA16 ~ -34.8733 (Annex B example)
        .i16(1000) // D23
        .i32(CENTER_LAT_RAW) // D24
        .u32(CENTER_LON_RAW) // D25
        .u16(0x0140) // D26 B16 2.5 km
        .u16(0x0200) // D27 BA16 2.8125
        .u16(0x8000) // D28
        .i16(0x1000) // D29
        .i16(-0x1000) // D30
        .u8(25); // D31
    for (i, (dla, dlo)) in [(100i16, -200i16), (-7, 3)].into_iter().enumerate() {
        b = b
            .u16(7 + i as u16) // D32.1
            .i16(dla) // D32.4
            .i16(dlo) // D32.5
            .i16(-12) // D32.6 m
            .i16(-420) // D32.7 cm/s
            .u16(3010) // D32.8 cm/s
            .i8(14) // D32.9
            .u8(2) // D32.10 wheeled
            .u8(80) // D32.11
            .u16(1000) // D32.12 cm
            .u16(250) // D32.13 dm
            .u8(5) // D32.14 m
            .u16(50) // D32.15 cm/s
            .u8(1) // D32.16
            .u32(42) // D32.17
            .i8(11); // D32.18 dB/2
    }
    b.0
}

/// Mandatory dwell fields, sensor heading only, and high-resolution targets.
/// Mask: FF 00 03 E1 80 00 00 00 (D28 + D32.2 + D32.3).
fn dwell_hires_body() -> Vec<u8> {
    B::new()
        .bytes(&[0xFF, 0x00, 0x03, 0xE1, 0x80, 0, 0, 0])
        .u16(0)
        .u16(0)
        .u8(1)
        .u16(2)
        .u32(90_061_001) // next day 01:01:01.001
        .sa32(51.0)
        .ba32(359.0)
        .i32(-2000)
        .sa32(51.5)
        .ba32(359.5)
        .u16(0x0080) // 1 km
        .u16(0x0100)
        .u16(0x2000) // D28 = 45 deg; D29/D30 absent
        .sa32(51.4)
        .ba32(359.0)
        .i32(i32::MIN) // placeholder location (-90, 180)
        .u32(0x8000_0000)
        .0
}

fn full_stream() -> Vec<u8> {
    let mut s = packet(
        0,
        &[
            segment(1, &mission_body()),
            segment(5, &job_definition_body(5)),
        ],
    );
    s.extend(packet(5, &[segment(2, &dwell_delta_body())]));
    s.extend(packet(5, &[segment(2, &dwell_hires_body())]));
    s
}

#[test]
fn version_and_options() {
    assert_eq!(VERSION, "stanag4607-2");
    let o: Options = serde_json::from_str(r#"{"emit":"all"}"#).unwrap();
    assert_eq!(o.emit, Emit::All);
    let d: Options = serde_json::from_str("{}").unwrap();
    assert_eq!(d.emit, Emit::Targets);
    assert!(serde_json::from_str::<Options>(r#"{"emit":"x"}"#).is_err());
    assert!(serde_json::from_str::<Options>(r#"{"other":1}"#).is_err());
}

#[test]
fn packet_header_and_mission() {
    let p = packet(0, &[segment(1, &mission_body())]);
    assert_eq!(packet_len(&p), Some(p.len()));
    assert_eq!(packet_len(&p[..5]), None);
    let (pkt, used) = parse_packet(&p).unwrap();
    assert_eq!(used, p.len());
    let h = &pkt.header;
    assert_eq!(h.version, "30");
    assert_eq!(h.classification.name, "UNCLASSIFIED");
    assert_eq!(h.code_names, vec!["NOCONTRACT", "REL NATO"]);
    assert!(h.is_exercise());
    assert_eq!(h.exercise_indicator.name, "Exercise, Simulated Data");
    assert_eq!(h.platform_id, "TAIL01");
    let Segment::Mission(m) = &pkt.segments[0] else {
        panic!()
    };
    assert_eq!(m.mission_plan, "MSN-PLAN-1");
    assert_eq!(m.platform_type.name, "Global Hawk-Australia");
    assert_eq!(m.reference_date.as_deref(), Some("2024-02-28"));
}

#[test]
fn dwell_delta_form_all_fields() {
    let (pkt, _) = parse_packet(&packet(5, &[segment(2, &dwell_delta_body())])).unwrap();
    let Segment::Dwell(d) = &pkt.segments[0] else {
        panic!("{:?}", pkt.segments)
    };
    assert_eq!(d.existence_mask, "fffffffe7fff0000");
    assert_eq!(
        (d.revisit_index, d.dwell_index, d.last_dwell_of_revisit),
        (3, 4, true)
    );
    assert!(close(d.sensor_lat, sa32(SENSOR_LAT_RAW)));
    assert!((d.sensor_lon + 10.0).abs() < 1e-7);
    assert!(close(d.sensor_alt_m, 10_000.0));
    assert!(close(d.sensor_along_track_uncertainty_m.unwrap(), 1.5));
    assert!(close(d.sensor_cross_track_uncertainty_m.unwrap(), 2.5));
    assert!(close(d.sensor_alt_uncertainty_m.unwrap(), 3.0));
    assert!(close(d.sensor_track_deg.unwrap(), 90.0));
    assert!(close(d.sensor_speed_mps.unwrap(), 200.0));
    assert!(close(d.sensor_vertical_velocity_mps.unwrap(), -0.5));
    assert!(close(d.sensor_speed_uncertainty_mps.unwrap(), 1.5));
    assert!(close(
        d.sensor_vertical_velocity_uncertainty_mps.unwrap(),
        0.2
    ));
    // Binary angles (Annex B 6.6/6.7 worked examples).
    assert!((d.platform_heading_deg.unwrap() - 125.31006).abs() < 1e-5);
    assert!((d.platform_pitch_deg.unwrap() + 34.8733).abs() < 3e-3);
    // Scaled integer: B16 0x0140 = 2.5 km.
    assert!(close(d.range_half_extent_m, 2500.0));
    assert!(close(d.dwell_angle_half_extent_deg, 2.8125));
    assert!(close(d.sensor_heading_deg.unwrap(), 180.0));
    assert!(close(d.sensor_pitch_deg.unwrap(), 11.25));
    assert!(close(d.sensor_roll_deg.unwrap(), -11.25));
    assert!(close(d.mdv_mps.unwrap(), 2.5));
    assert_eq!(d.targets.len(), 2);

    let t = &d.targets[0];
    assert_eq!(t.mti_report_index, Some(7));
    assert!(t.hi_res_lat.is_none());
    let lat = 100.0 * sa32(LAT_SCALE_RAW) + sa32(CENTER_LAT_RAW);
    let lon = -200.0 * ba32(LON_SCALE_RAW) + ba32(CENTER_LON_RAW) - 360.0;
    assert!(close(t.lat.unwrap(), lat));
    assert!(close(t.lon.unwrap(), lon));
    assert!(close(t.height_m.unwrap(), -12.0));
    assert!(close(t.radial_velocity_mps.unwrap(), -4.2));
    assert!(close(t.wrap_velocity_mps.unwrap(), 30.1));
    assert_eq!(
        t.classification.unwrap().name,
        "Wheeled Vehicle, Live Target"
    );
    assert!(close(t.slant_range_uncertainty_m.unwrap(), 10.0));
    assert!(close(t.cross_range_uncertainty_m.unwrap(), 25.0));
    assert!(close(t.radial_velocity_uncertainty_mps.unwrap(), 0.5));
    assert_eq!(
        (t.truth_tag_application, t.truth_tag_entity),
        (Some(1), Some(42))
    );
    assert!(close(t.rcs_dbsm.unwrap(), 5.5));
    assert_eq!(d.targets[1].report_index, 1);
}

#[test]
fn target_records_with_context() {
    let recs = Decoder::new(Options::default())
        .decode(&full_stream())
        .unwrap();
    assert_eq!(recs.len(), 4, "{recs:#?}");
    let r = &recs[0];
    assert_eq!(r["record"], "target");
    assert_eq!(r["time"], "2024-02-28T01:00:00.500Z");
    assert_eq!(r["time_basis"], "mission_reference_date");
    assert_eq!(r["classification"]["probability"], 0.8);
    assert_eq!(r["classification"]["code"], 2);
    assert_eq!(r["location_error"]["source"], "target_report");
    assert_eq!(r["location_error"]["cross_range_m"], 25.0);
    assert_eq!(r["location_error"]["height_m"], 5.0);
    assert_eq!(r["snr_db"], 14.0);
    assert_eq!(r["rcs_dbsm"], 5.5);
    assert_eq!(r["report_index"], 0);
    assert_eq!(r["mti_report_index"], 7);
    assert_eq!(r["dwell"]["target_count"], 2);
    assert_eq!(r["dwell"]["sensor"]["speed_mps"], 200.0);
    assert_eq!(r["dwell"]["area"]["range_half_extent_m"], 2500.0);
    assert_eq!(
        r["dwell"]["platform_orientation"]["heading_deg"],
        125.31005859375
    );
    assert_eq!(r["job_id"], 5);
    assert_eq!(r["job"]["sensor_type"]["name"], "MP-RTIP");
    assert_eq!(r["job"]["sensor_model"], "MOD-A");
    assert_eq!(r["job"]["area_filtering"], true);
    assert_eq!(r["job"]["radar_priority"], 10);
    assert_eq!(r["job"]["nominal_along_track_uncertainty_m"], 5.0);
    assert!(
        r["job"].get("nominal_cross_track_uncertainty_m").is_none(),
        "No Statement omitted"
    );
    assert_eq!(r["mission"]["platform_type"], "Global Hawk-Australia");
    assert_eq!(r["mission"]["reference_date"], "2024-02-28");
    assert_eq!(r["packet"]["exercise"], true);
    assert_eq!(r["packet"]["classification"], "UNCLASSIFIED");
    assert!(r.as_object().unwrap().values().all(|v| !v.is_null()));

    // High-resolution dwell: no target uncertainty -> job nominal values.
    let h = &recs[2];
    assert_eq!(h["time"], "2024-02-29T01:01:01.001Z");
    assert!((h["lat"].as_f64().unwrap() - 51.4).abs() < 1e-7);
    assert!((h["lon"].as_f64().unwrap() + 1.0).abs() < 1e-7);
    assert_eq!(h["location_error"]["source"], "job_nominal");
    assert_eq!(h["location_error"]["slant_range_m"], 15.0);
    assert_eq!(h["location_error"]["cross_range_deg"], 1.40625);
    assert_eq!(h["location_error"]["radial_velocity_mps"], 0.4);
    assert!(h.get("height_m").is_none() && h.get("mti_report_index").is_none());
    // Sensor orientation: D28 only, omitted angles are zero (para 2.4.28).
    assert_eq!(h["dwell"]["sensor_orientation"]["heading_deg"], 45.0);
    assert_eq!(h["dwell"]["sensor_orientation"]["pitch_deg"], 0.0);
    assert!(h["dwell"].get("platform_orientation").is_none());
    // Placeholder (-90, 180) location resolves to no position.
    assert!(recs[3].get("lat").is_none() && recs[3].get("lon").is_none());
}

#[test]
fn emit_modes() {
    let s = full_stream();
    let seg = Decoder::new(Options {
        emit: Emit::Segments,
        ..Default::default()
    })
    .decode(&s)
    .unwrap();
    let kinds: Vec<_> = seg.iter().map(|r| r["record"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["mission", "job_definition", "dwell", "dwell"]);
    assert_eq!(seg[2]["targets"].as_array().unwrap().len(), 2);
    assert_eq!(seg[2]["time"], "2024-02-28T01:00:00.500Z");
    assert_eq!(seg[2]["packet"]["job_id"], 5);
    let all = Decoder::new(Options {
        emit: Emit::All,
        ..Default::default()
    })
    .decode(&s)
    .unwrap();
    let kinds: Vec<_> = all.iter().map(|r| r["record"].as_str().unwrap()).collect();
    assert_eq!(
        kinds,
        [
            "mission",
            "job_definition",
            "dwell",
            "target",
            "target",
            "dwell",
            "target",
            "target"
        ]
    );
}

#[test]
fn time_without_mission_uses_receipt_clock() {
    let recs = Decoder::new(Options::default())
        .decode(&packet(5, &[segment(2, &dwell_delta_body())]))
        .unwrap();
    assert_eq!(recs[0]["time_basis"], "receipt_clock_time_of_day");
    let t = chrono::DateTime::parse_from_rfc3339(recs[0]["time"].as_str().unwrap()).unwrap();
    assert!(
        (t.with_timezone(&chrono::Utc) - chrono::Utc::now())
            .num_hours()
            .abs()
            <= 12
    );
    assert!(recs[0].get("mission").is_none() && recs[0].get("job").is_none());
    assert!(recs[0]["time"].as_str().unwrap().contains("T01:00:00.500"));
}

#[test]
fn missing_mandatory_field_is_a_warning_and_packet_continues() {
    // D24 cleared from the mask.
    let mut body = dwell_hires_body();
    body[2] = 0x01;
    let ft = B::new().a("ORIG", 10).a("RCPT", 10).bytes(b"hello world").0;
    let p = packet(5, &[segment(2, &body), segment(6, &ft)]);
    let recs = Decoder::new(Options {
        emit: Emit::All,
        ..Default::default()
    })
    .decode(&p)
    .unwrap();
    assert_eq!(recs[0]["record"], "warning");
    assert_eq!(recs[0]["segment_name"], "dwell");
    assert!(recs[0]["error"].as_str().unwrap().contains("D24"));
    assert_eq!(recs[1]["record"], "free_text");
    assert_eq!(recs[1]["originator_id"], "ORIG");
    assert_eq!(recs[1]["text"], "hello world");
}

fn hrr_full_body() -> Vec<u8> {
    let mut b = B::new()
        .bytes(&[0xFF, 0xFF, 0xFF, 0xFF, 0xC0])
        .u16(1)
        .u16(2)
        .u8(1)
        .u16(7)
        .u16(2)
        .u16(16)
        .u16(8)
        .u8(115) // H9 -28.75 dB
        .u8(40) // H10 -10 dB
        .u16(0x0140) // H11 B16 2.5 cm
        .u16(0x0100) // H12 2 cm
        .u32(0x0001_8000) // H13 H32 1.5 Hz
        .u32(0x8002_0000) // H14 H32 -2.0 Hz
        .u32(0x0500_0000) // H15 B32 10 GHz
        .u8(1)
        .u8(1)
        .u8(2)
        .u16(0x8280) // H19 B16 -5 dB
        .i8(-6) // H20 -3 dBsm
        .i16(-100) // H21
        .u32(0x0010_0000) // H22 16 Hz
        .u8(2) // H23
        .u8(0xC0) // H24
        .u8(2) // H25
        .u8(2) // H26
        .u8(16)
        .u32(12345)
        .u8(4)
        .u32(0x0280_0000) // H30 5 m
        .u32(0x0040_0000); // H31 0.5 m
    for i in 0..2u16 {
        b = b.u16(400 + i).u16(0x4000).u16(3 + i).u16(5 + i);
    }
    b.0
}

#[test]
fn hrr_full_and_minimal() {
    let (pkt, _) = parse_packet(&packet(5, &[segment(3, &hrr_full_body())])).unwrap();
    let Segment::Hrr(h) = &pkt.segments[0] else {
        panic!("{:?}", pkt.segments)
    };
    assert_eq!(h.existence_mask, "ffffffffc0");
    assert_eq!(h.mti_report_index, Some(7));
    assert_eq!(h.mean_clutter_power_db, Some(-28.75));
    assert_eq!(h.detection_threshold_db, Some(-10.0));
    assert_eq!(h.range_resolution_m, Some(0.025));
    assert_eq!(h.doppler_resolution_hz, Some(1.5));
    assert_eq!(h.doppler_bin_spacing_hz, Some(-2.0));
    assert_eq!(h.center_frequency_ghz, Some(10.0));
    assert_eq!(h.maximum_pixel_power_db, Some(-5.0));
    assert_eq!(h.maximum_rcs_dbsm, Some(-3.0));
    assert_eq!(h.range_of_origin_m, Some(-100.0));
    assert_eq!(h.doppler_of_origin_hz, Some(16.0));
    assert_eq!(h.hrr_type.unwrap().name, "2-D HRR Chip");
    assert_eq!(
        h.processing_applied,
        vec!["Clutter Cancellation", "Single-Ambiguity Keystoning"]
    );
    assert_eq!(h.range_to_nearest_edge_m, Some(123.45));
    assert_eq!(h.target_radial_electrical_length_m, Some(5.0));
    assert_eq!(h.electrical_length_uncertainty_m, Some(0.5));
    assert_eq!(h.scatterers.len(), 2);
    assert_eq!(h.scatterers[1].magnitude, 401);
    assert_eq!(h.scatterers[1].phase_deg, Some(90.0));
    assert_eq!(h.scatterers[1].range_index, Some(4));
    assert_eq!(h.scatterers[1].doppler_index, Some(6));

    // Mandatory fields only: 1-byte magnitudes, no phase or indices.
    let body = B::new()
        .bytes(&[0xEA, 0xFB, 0xC7, 0x82, 0x00])
        .u16(1)
        .u16(0)
        .u8(1)
        .u16(3) // H6
        .u16(8) // H8
        .u8(0)
        .u16(0)
        .u16(0) // H12 no statement
        .u32(0)
        .u32(0)
        .u8(0)
        .u8(0)
        .u8(0)
        .u16(0)
        .u8(1)
        .u8(0)
        .u8(1) // H25
        .u8(0) // H26
        .bytes(&[10, 20, 30])
        .0;
    let (pkt, _) = parse_packet(&packet(5, &[segment(3, &body)])).unwrap();
    let Segment::Hrr(h) = &pkt.segments[0] else {
        panic!("{:?}", pkt.segments)
    };
    assert_eq!(h.num_target_scatterers, Some(3));
    assert_eq!(h.num_range_samples, None);
    assert_eq!(h.center_frequency_ghz, None);
    assert_eq!(h.range_bin_spacing_m, None);
    let mags: Vec<_> = h.scatterers.iter().map(|s| s.magnitude).collect();
    assert_eq!(mags, [10, 20, 30]);
    assert!(
        h.scatterers
            .iter()
            .all(|s| s.phase.is_none() && s.range_index.is_none())
    );
}

#[test]
fn job_definition_fields() {
    let (pkt, _) = parse_packet(&packet(0, &[segment(5, &job_definition_body(9))])).unwrap();
    let Segment::JobDefinition(j) = &pkt.segments[0] else {
        panic!()
    };
    assert_eq!(j.job_id, 9);
    assert!(j.area_filtering && !j.area_blanking && j.sector_blanking);
    assert!((j.bounding_area.points[2][0] - 11.0).abs() < 1e-7);
    assert!((j.bounding_area.points[2][1] - 21.0).abs() < 1e-7);
    assert_eq!(j.radar_mode.name, "MTI (Moving Target Indicator)");
    assert_eq!(j.nominal_revisit_interval_s, 12.5);
    assert_eq!(j.nominal_cross_track_uncertainty_m, None);
    assert_eq!(j.nominal_alt_uncertainty_m, Some(3.0));
    assert_eq!(j.nominal_speed_uncertainty_mps, Some(0.25));
    assert_eq!(j.nominal_mdv_mps, Some(1.5));
    assert_eq!(j.nominal_detection_probability_pct, Some(85));
    assert_eq!(j.terrain_elevation_model.name, "DTED2");
    assert_eq!(j.geoid_model.name, "EGM96");
}

#[test]
fn other_segment_types() {
    let ts = B::new()
        .u32(5)
        .u16(1)
        .u16(2)
        .u32(3_600_000)
        .u8(0xA0)
        .u8(0x10)
        .0;
    let ph = B::new()
        .u8(2)
        .a("US", 2)
        .a("ORIG", 10)
        .u32(1)
        .u32(2)
        .u8(1)
        .a("UK", 2)
        .a("PROC1", 10)
        .u32(3)
        .u32(4)
        .u16(0x2001)
        .u8(2)
        .a("FR", 2)
        .a("PROC2", 10)
        .u32(5)
        .u32(6)
        .u16(0x0100)
        .0;
    let pl = B::new()
        .u32(7_200_000)
        .sa32(45.0)
        .ba32(200.0)
        .i32(900_000)
        .u16(0x2000)
        .u32(150_000)
        .i8(12)
        .0;
    let area = |b: B| {
        b.sa32(1.0)
            .ba32(2.0)
            .sa32(3.0)
            .ba32(4.0)
            .sa32(5.0)
            .ba32(6.0)
            .sa32(7.0)
            .ba32(8.0)
    };
    let jr = area(B::new().a("REQ", 10).a("TASK-1", 10).u8(3))
        .u8(1)
        .u16(150)
        .u16(25)
        .u16(2024)
        .u8(3)
        .u8(1)
        .u8(12)
        .u8(30)
        .u8(0)
        .u16(60)
        .u16(0)
        .u16(100)
        .u8(255)
        .a("None", 6)
        .u8(1)
        .0;
    let ja = area(
        B::new()
            .u32(5)
            .a("REQ", 10)
            .a("TASK-1", 10)
            .u8(7)
            .a("M", 6)
            .u8(4),
    )
    .u8(1)
    .u16(600)
    .u16(50)
    .u8(2)
    .u16(2024)
    .u8(3)
    .u8(1)
    .u8(12)
    .u8(30)
    .u8(60)
    .a("US", 2)
    .0;
    let segs = vec![
        segment(10, &ts),
        segment(12, &ph),
        segment(13, &pl),
        segment(101, &jr),
        segment(102, &ja),
        segment(4, &[1, 2, 3]),
        segment(7, &[]),
        segment(8, &[1]),
        segment(9, &[1, 2]),
        segment(11, &[9; 7]),
        segment(50, &[0; 4]),
        segment(200, &[0; 2]),
    ];
    let p = packet(0, &segs);
    let (pkt, _) = parse_packet(&p).unwrap();
    let sum: usize = pkt.segments.iter().map(Segment::segment_size).sum();
    assert_eq!(sum + 32, p.len());

    let Segment::TestAndStatus(t) = &pkt.segments[0] else {
        panic!()
    };
    assert!(t.antenna_fail && t.processor_fail && !t.rf_electronics_fail);
    assert!(t.temperature_limit_exceeded && !t.range_limit_exceeded);
    let Segment::ProcessingHistory(h) = &pkt.segments[1] else {
        panic!()
    };
    assert_eq!(h.records.len(), 2);
    assert_eq!(
        h.records[0].processing_performed_names,
        vec!["Area Filtering", "Target Coordinate Conversion"]
    );
    assert_eq!(h.records[1].platform_id, "PROC2");
    let Segment::PlatformLocation(l) = &pkt.segments[2] else {
        panic!()
    };
    assert!((l.lat - 45.0).abs() < 1e-7 && (l.lon + 160.0).abs() < 1e-7);
    assert_eq!(
        (l.alt_m, l.track_deg, l.speed_mps, l.vertical_velocity_mps),
        (9000.0, 45.0, 150.0, 1.2)
    );
    let Segment::JobRequest(r) = &pkt.segments[3] else {
        panic!()
    };
    assert_eq!(r.requestor_task_id, "TASK-1");
    assert_eq!(r.range_resolution_m, Some(1.5));
    assert_eq!(r.cross_range_resolution_m, Some(2.5));
    assert_eq!(
        r.earliest_start_time.as_deref(),
        Some("2024-03-01T12:30:00Z")
    );
    assert_eq!(r.revisit_interval_s, 10.0);
    assert_eq!(r.sensor_type.name, "No Statement");
    assert!(r.cancel);
    let Segment::JobAcknowledge(a) = &pkt.segments[4] else {
        panic!()
    };
    assert_eq!(a.request_status.name, "Approved, with Modification");
    assert_eq!(a.start_time.as_deref(), Some("2024-03-01T12:31:00Z"));
    assert_eq!(a.sensor_type.name, "APY-3");
    assert!((a.bounding_area.points[3][0] - 7.0).abs() < 1e-7);
    assert!(matches!(pkt.segments[5], Segment::RangeDoppler(_)));
    assert!(matches!(pkt.segments[6], Segment::LowReflectivityIndex(_)));
    assert!(matches!(pkt.segments[7], Segment::Group(_)));
    assert!(matches!(pkt.segments[8], Segment::AttachedTarget(_)));
    assert!(matches!(pkt.segments[9], Segment::SystemSpecific(_)));
    assert_eq!(
        pkt.segments[10],
        Segment::Unknown {
            segment_type: 50,
            bytes_len: 9
        }
    );
    assert_eq!(
        pkt.segments[11],
        Segment::Unknown {
            segment_type: 200,
            bytes_len: 7
        }
    );

    let recs = Decoder::new(Options {
        emit: Emit::Segments,
        ..Default::default()
    })
    .decode(&p)
    .unwrap();
    let kinds: Vec<_> = recs.iter().map(|r| r["record"].as_str().unwrap()).collect();
    assert_eq!(
        kinds,
        [
            "test_and_status",
            "processing_history",
            "platform_location",
            "job_request",
            "job_acknowledge",
            "range_doppler",
            "low_reflectivity_index",
            "group",
            "attached_target",
            "system_specific",
            "unknown",
            "unknown"
        ]
    );
    assert_eq!(recs[0]["time_basis"], "receipt_clock_time_of_day");
    assert!((recs[2]["lon"].as_f64().unwrap() + 160.0).abs() < 1e-7);
    // Only targets are emitted by default, and there are none here.
    assert!(
        Decoder::new(Options::default())
            .decode(&p)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn bad_sizes() {
    // Truncated job definition body inside a well-formed packet: warning.
    let p = packet(0, &[segment(5, &job_definition_body(1)[..20])]);
    let recs = Decoder::new(Options::default()).decode(&p).unwrap();
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0]["record"], "warning");
    // Segment size runs past the packet end: warning, parsing of the packet stops.
    let mut seg = segment(6, &B::new().a("A", 10).a("B", 10).0);
    seg[4] = 200;
    let recs = Decoder::new(Options::default())
        .decode(&packet(0, &[seg]))
        .unwrap();
    assert!(recs[0]["error"].as_str().unwrap().contains("segment size"));
    // Segment size below the segment header size.
    let recs = Decoder::new(Options::default())
        .decode(&packet(0, &[vec![6, 0, 0, 0, 2]]))
        .unwrap();
    assert_eq!(recs[0]["record"], "warning");
    // Packet size smaller than the header: error.
    let mut p = packet(0, &[]);
    p[5] = 10;
    assert!(matches!(parse_packet(&p), Err(Error::BadPacket(_))));
    // Trailing partial packet: Truncated.
    let mut two = packet(0, &[segment(1, &mission_body())]);
    let one = two.len();
    two.extend(packet(0, &[segment(1, &mission_body())]));
    two.truncate(one + 40);
    assert!(matches!(
        Decoder::new(Options::default()).decode(&two),
        Err(Error::Truncated { need, have: 40 }) if need == one
    ));
    assert!(
        Decoder::new(Options::default())
            .decode(&[])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn platform_records_from_dwells_and_platform_locations() {
    let pl = B::new()
        .u32(3_601_000)
        .sa32(45.0)
        .ba32(200.0)
        .i32(900_000)
        .u16(0x2000)
        .u32(150_000)
        .i8(12)
        .0;
    let mut s = full_stream();
    s.extend(packet(5, &[segment(13, &pl)]));
    let off = Decoder::new(Options::default()).decode(&s).unwrap();
    assert!(off.iter().all(|r| r["record"] != "platform"));

    let recs = Decoder::new(Options {
        platform: true,
        ..Default::default()
    })
    .decode(&s)
    .unwrap();
    let kinds: Vec<_> = recs.iter().map(|r| r["record"].as_str().unwrap()).collect();
    assert_eq!(
        kinds,
        [
            "platform", "target", "target", "platform", "target", "target", "platform"
        ]
    );
    let d = &recs[0];
    assert_eq!(d["from"], "dwell");
    assert_eq!(d["time"], "2024-02-28T01:00:00.500Z");
    assert!(close(d["lat"].as_f64().unwrap(), sa32(SENSOR_LAT_RAW)));
    assert!((d["lon"].as_f64().unwrap() + 10.0).abs() < 1e-7);
    assert!(close(d["alt_m"].as_f64().unwrap(), 10_000.0));
    assert!(close(d["track_deg"].as_f64().unwrap(), 90.0));
    assert!(close(d["speed_mps"].as_f64().unwrap(), 200.0));
    assert!(close(
        d["uncertainty"]["along_track_m"].as_f64().unwrap(),
        1.5
    ));
    assert_eq!(d["platform_id"], recs[1]["packet"]["platform_id"]);
    assert!(d["mission"]["reference_date"].is_string());
    let l = &recs[6];
    assert_eq!(l["from"], "platform_location");
    assert_eq!(l["time"], "2024-02-28T01:00:01.000Z");
    assert!((l["lat"].as_f64().unwrap() - 45.0).abs() < 1e-7);
    assert!((l["lon"].as_f64().unwrap() + 160.0).abs() < 1e-7);
    assert_eq!(l["alt_m"], 9000.0);
    assert_eq!(l["speed_mps"], 150.0);
    assert!(l.get("uncertainty").is_none());
}

/// The job definition body with J21 (slant range std) and J22 (cross-range
/// std) replaced; 0xFFFF is No Statement for both.
fn job_with_accuracy(j21: u16, j22: u16) -> Vec<u8> {
    let mut b = job_definition_body(5);
    b[57..59].copy_from_slice(&j21.to_be_bytes());
    b[59..61].copy_from_slice(&j22.to_be_bytes());
    b
}

fn hires_stream(job: Option<Vec<u8>>) -> Vec<u8> {
    let mut first = vec![segment(1, &mission_body())];
    first.extend(job.map(|j| segment(5, &j)));
    let mut s = packet(0, &first);
    s.extend(packet(5, &[segment(2, &dwell_hires_body())]));
    s
}

fn f(v: &serde_json::Value, k: &str) -> f64 {
    v[k].as_f64().unwrap_or_else(|| panic!("{k} in {v}"))
}

#[test]
fn ground_error_from_the_target_report() {
    let recs = Decoder::new(Options::default())
        .decode(&full_stream())
        .unwrap();
    let r = &recs[0];
    let g = &r["ground_error"];
    assert_eq!(g["source"], "target_report", "{g}");
    // D32.13 is already a distance: used as is.
    assert_eq!(f(g, "cross_range_sigma_m"), 25.0);
    // D32.12 projected to the ground: sigma_R / cos(grazing).
    let (rho, slant) = (f(g, "ground_range_m"), f(g, "slant_range_m"));
    let dh = 10_000.0 - -12.0;
    assert!((slant - rho.hypot(dh)).abs() < 1e-6);
    assert!((f(g, "range_sigma_m") - 10.0 * slant / rho).abs() < 1e-9);
    assert_eq!(f(g, "radial_velocity_sigma_mps"), 0.5);
    assert!(f(g, "semi_major_m") >= f(g, "semi_minor_m"));
    let o = f(g, "orientation_deg");
    assert!((0.0..180.0).contains(&o));
    // The range axis lies along the bearing; here the cross range (25 m)
    // dominates, so the major axis is across it.
    assert!(f(g, "range_sigma_m") < 25.0);
    assert_eq!(f(g, "semi_major_m"), 25.0);
    let d = (o - f(g, "bearing_deg") - 90.0).rem_euclid(180.0);
    assert!(d.min(180.0 - d) < 1e-6, "{g}");
    for k in ["nn_m2", "ne_m2", "ee_m2"] {
        assert!(g[k].as_f64().unwrap().is_finite());
    }
}

#[test]
fn ground_error_from_the_job_nominal_values() {
    let recs = Decoder::new(Options {
        range_sigma_m: Some(99.0),
        cross_range_sigma_deg: Some(9.0),
        ..Default::default()
    })
    .decode(&full_stream())
    .unwrap();
    // The report's own values beat the options too.
    assert_eq!(recs[0]["ground_error"]["source"], "target_report");
    // Target due north of the sensor (51.0 -> 51.4, same longitude).
    let g = &recs[2]["ground_error"];
    assert_eq!(g["source"], "job_nominal", "{g}");
    let (rho, slant) = (f(g, "ground_range_m"), f(g, "slant_range_m"));
    assert!((rho - 44_500.0).abs() < 200.0, "{g}");
    assert!((slant - rho.hypot(-20.0)).abs() < 1e-6);
    assert!((f(g, "range_sigma_m") - 15.0 * slant / rho).abs() < 1e-9);
    let sx = slant * 1.40625f64.to_radians();
    assert!((f(g, "cross_range_sigma_m") - sx).abs() < 1e-6);
    // Cross range (east-west) dominates.
    assert!((f(g, "orientation_deg") - 90.0).abs() < 1e-6, "{g}");
    assert!((f(g, "semi_major_m") - sx).abs() < 1e-6);
    assert!(f(g, "bearing_deg") < 1e-6 || f(g, "bearing_deg") > 360.0 - 1e-6);
    assert_eq!(f(g, "radial_velocity_sigma_mps"), 0.4);
    // No location, no ground error.
    assert!(recs[3].get("ground_error").is_none());
}

#[test]
fn ground_error_from_the_options() {
    // No job definition, no report accuracy: nothing without options.
    let s = hires_stream(None);
    let recs = Decoder::new(Options::default()).decode(&s).unwrap();
    assert!(recs[0].get("lat").is_some());
    assert!(recs[0].get("ground_error").is_none(), "{}", recs[0]);
    // One option alone is not enough.
    let recs = Decoder::new(Options {
        range_sigma_m: Some(20.0),
        ..Default::default()
    })
    .decode(&s)
    .unwrap();
    assert!(recs[0].get("ground_error").is_none());
    // Non-positive options are ignored.
    let recs = Decoder::new(Options {
        range_sigma_m: Some(0.0),
        cross_range_sigma_deg: Some(0.2),
        ..Default::default()
    })
    .decode(&s)
    .unwrap();
    assert!(recs[0].get("ground_error").is_none());

    let opts = Options {
        range_sigma_m: Some(20.0),
        cross_range_sigma_deg: Some(0.2),
        ..Default::default()
    };
    let recs = Decoder::new(opts.clone()).decode(&s).unwrap();
    let g = &recs[0]["ground_error"];
    assert_eq!(g["source"], "options", "{g}");
    let (rho, slant) = (f(g, "ground_range_m"), f(g, "slant_range_m"));
    assert!((f(g, "range_sigma_m") - 20.0 * slant / rho).abs() < 1e-9);
    assert!((f(g, "cross_range_sigma_m") - slant * 0.2f64.to_radians()).abs() < 1e-6);
    assert!(g.get("radial_velocity_sigma_mps").is_none());

    // Job gives a slant range sigma but no cross range (J22 No Statement):
    // mixed job + options is reported as the less specific, options.
    let recs = Decoder::new(opts.clone())
        .decode(&hires_stream(Some(job_with_accuracy(1500, 0xFFFF))))
        .unwrap();
    let g = &recs[0]["ground_error"];
    assert!(recs[0]["job"].get("nominal_cross_range_std_deg").is_none());
    assert_eq!(g["source"], "options", "{g}");
    let (rho, slant) = (f(g, "ground_range_m"), f(g, "slant_range_m"));
    assert!((f(g, "range_sigma_m") - 15.0 * slant / rho).abs() < 1e-9);
    assert!((f(g, "cross_range_sigma_m") - slant * 0.2f64.to_radians()).abs() < 1e-6);

    // Job without J21: the option fills the range sigma.
    let recs = Decoder::new(opts)
        .decode(&hires_stream(Some(job_with_accuracy(0xFFFF, 0x0100))))
        .unwrap();
    let g = &recs[0]["ground_error"];
    assert!(recs[0]["job"].get("nominal_slant_range_std_m").is_none());
    assert_eq!(g["source"], "options");
    assert!(
        (f(g, "range_sigma_m") - 20.0 * f(g, "slant_range_m") / f(g, "ground_range_m")).abs()
            < 1e-9
    );
    assert!(
        (f(g, "cross_range_sigma_m") - f(g, "slant_range_m") * 1.40625f64.to_radians()).abs()
            < 1e-6
    );
}

#[test]
fn sigma_options_deserialise() {
    let o: Options =
        serde_json::from_str(r#"{"range_sigma_m": 20.0, "cross_range_sigma_deg": 0.2}"#).unwrap();
    assert_eq!(o.range_sigma_m, Some(20.0));
    assert_eq!(o.cross_range_sigma_deg, Some(0.2));
    assert_eq!(Options::default().range_sigma_m, None);
}

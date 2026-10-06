use chrono::Utc;
use ot_core::Geometry;
use ot_source::mapping::{MapOutcome, finalize};
use ot_source::source::SourceSpec;
use serde_json::json;

#[test]
fn acoustic_example_maps_a_sapient_bearing() {
    let source: SourceSpec = serde_json::from_str(include_str!(
        "../../../docs/examples/sapient-acoustic-mqtt.json"
    ))
    .expect("valid source JSON");
    source.validate().expect("valid source specification");

    let record = json!({
        "timestamp": "2026-10-01T12:00:00+00:00",
        "node_id": "00000000-0000-4000-8000-000000000001",
        "detection_report": {
            "object_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "id": "quadcopter-alpha",
            "detection_confidence": 0.9,
            "range_bearing": {
                "azimuth": 42.0,
                "azimuth_error": 1.2,
                "elevation": 8.0,
                "range": 300.0,
                "range_error": 12.0
            }
        }
    });
    let MapOutcome::Mapped(mapped) = source.pipeline.mapping.apply(&record) else {
        panic!("acoustic record did not match the source mapping");
    };
    let observation = finalize(&source.id, 1, &mapped[0], Utc::now()).expect("valid bearing");

    assert_eq!(observation.source_track_key, "01ARZ3NDEKTSV4RRFFQ69G5FAV");
    // A node's object id is its own track key, not an identity: each array
    // numbers the drones it hears itself, so as an identifier it would veto
    // fusing one drone's reports from several arrays.
    assert!(observation.identifiers.is_empty());
    assert_eq!(observation.position.latitude, 51.5);
    assert_eq!(observation.position.longitude, -0.102);
    assert_eq!(
        observation.geometry,
        Some(Geometry::Bearing {
            bearing_deg: 42.0,
            sigma_deg: 1.2,
            range_m: Some(300.0),
            range_sigma_m: Some(12.0),
            max_range_m: Some(1000.0),
            elevation_deg: Some(8.0),
        })
    );
}

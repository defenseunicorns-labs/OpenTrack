# ot-codec-stanag4607

Decoder for STANAG 4607 Edition 3 GMTI (Ground Moving Target Indicator)
packets. It is pure Rust with no `unsafe`. Malformed input never makes it
panic: bad input returns an `Err`, or the bad segment is skipped and
reported in a `warning` record.

Field layouts, data types, existence masks and enumerations come from
Annex A (Tables 2-1 to 2-15, 3-1 and 3-2) and the data conventions in
Annex B para 6. The doc comments in `src/segments.rs` cite the table and
field id behind each field. The standard's text is not reproduced here.

## Source

This crate implements **STANAG 4607 Edition 3, *NATO Ground Moving Target
Indicator Format (GMTIF)*, published as AEDP-7 (2013)**. The
standard is distributed through the
[NSG Standards Registry](https://nsgreg.nga.mil/), which also lists its
successor, AEDP-4607 Edition A (2024), and the implementation guide
AEDP-4607.1. Get the standard from the registry. This repository holds only
an independent implementation: field names, data types and code values, with
references to the standard's tables. It does not copy the standard's text.

## API

```rust
use ot_codec_stanag4607::{Decoder, Emit, Options, packet_len, parse_packet, VERSION};

let mut dec = Decoder::new(Options { emit: Emit::Targets, ..Default::default() });
let records: Vec<serde_json::Value> = dec.decode(&bytes)?;   // whole packets, back to back
let size = packet_len(&header_bytes);                        // framing: P2 packet size
let (packet, used) = parse_packet(&bytes)?;                  // typed, one packet
```

- `VERSION` (`"stanag4607-2"`) changes whenever the output for the same input changes.
- `Options` deserialises from
  `{"emit": "targets" | "segments" | "all", "platform": false, "range_sigma_m": null, "cross_range_sigma_deg": null}`
  (every key optional). Unknown keys are rejected.
  - `range_sigma_m`: one-sigma slant range error (m), and
  - `cross_range_sigma_deg`: one-sigma cross-range (azimuth) error (degrees),

  are fallbacks for `ground_error` (below), used only for a sigma that neither
  the target report nor the job definition states. Values that are not
  positive are ignored.
- `Decoder` keeps state for one stream:
  - the latest mission segment (reference day, platform);
  - job definitions, by job id.
- `decode` returns `Err(Truncated)` when the input ends in a partial packet.
  A packet with a bad size returns `Err(BadPacket)`. A bad segment inside
  an otherwise good packet becomes a `warning` record, and the packet's other
  segments still decode.

### Typed model

`parse_packet` returns `Packet { header: PacketHeader, segments: Vec<Segment> }`.
`Segment` has a variant for every segment type in Table 2-2:

| Type | Variant | Segment |
|---|---|---|
| 1 | `Mission` | Mission |
| 2 | `Dwell` | Dwell, with target reports |
| 3 | `Hrr` | HRR / range-Doppler map, with scatterers |
| 4 | `RangeDoppler` | Reserved in Ed. 3, no layout |
| 5 | `JobDefinition` | Job definition |
| 6 | `FreeText` | Free text |
| 7 | `LowReflectivityIndex` | Reserved, no layout |
| 8 | `Group` | Reserved, no layout |
| 9 | `AttachedTarget` | Reserved, no layout |
| 10 | `TestAndStatus` | Test and status |
| 11 | `SystemSpecific` | Reserved, no layout |
| 12 | `ProcessingHistory` | Processing history |
| 13 | `PlatformLocation` | Platform location |
| 101 | `JobRequest` | Job request |
| 102 | `JobAcknowledge` | Job acknowledge |
| other | `Unknown { segment_type, bytes_len }` | Types 14-100, 103-127 and extensions 128-255 |

Segments that fail to decode become `Malformed { segment_type, bytes_len, error }`.

Values in the typed model are in engineering units:

- Angles are in degrees. BA16/BA32 and SA16/SA32 are binary angles.
- Distances are in metres, speeds in m/s, and power in dB or dBsm.
- B16, B32 and H32 are sign-magnitude binary decimals.
- Longitudes are normalised to [-180, 180).
- A mandatory field sent as "No Statement" becomes `None`.
- Enumerations are `{ code, name }`.

## Record contract

Fields that are absent (mask bit clear, No Statement, no mission or job seen
yet) are left out of the record. They are never written as `null`.

### `emit: targets` (default)

The decoder writes one record per dwell target report:

```jsonc
{ "record": "target",
  "time": "2015-10-13T00:15:10.123Z",      // RFC 3339 UTC, ms precision
  "time_basis": "mission_reference_date",  // or "receipt_clock_time_of_day" (see below)
  "lat": -34.85, "lon": 138.49,            // high-res D32.2/3, else centre + delta x scale
  "height_m": 4.0, "radial_velocity_mps": -3.26, "wrap_velocity_mps": 27.89,
  "snr_db": 14.0, "rcs_dbsm": 5.5,
  "classification": { "code": 2, "name": "Wheeled Vehicle, Live Target", "probability": 0.8 },
  "location_error": { "source": "target_report", "slant_range_m": 10.0, "cross_range_m": 25.0,
                      "height_m": 5.0, "radial_velocity_mps": 0.5 },
  "ground_error": { "semi_major_m": 25.0, "semi_minor_m": 10.17, "orientation_deg": 112.5,
                    "nn_m2": 179.8, "ne_m2": -184.4, "ee_m2": 548.6,
                    "range_sigma_m": 10.17, "cross_range_sigma_m": 25.0,
                    "slant_range_m": 55213.0, "ground_range_m": 54300.0, "bearing_deg": 22.5,
                    "source": "target_report", "radial_velocity_sigma_mps": 0.5 },
  "truth_tag": { "application": 1, "entity": 42 },
  "report_index": 0, "mti_report_index": 0,
  "dwell": { "revisit_index", "dwell_index", "last_dwell_of_revisit", "time_ms", "target_count", "mdv_mps",
             "sensor": { "lat", "lon", "alt_m", "track_deg", "speed_mps", "vertical_velocity_mps",
                         "along_track_uncertainty_m", "cross_track_uncertainty_m", "alt_uncertainty_m",
                         "track_uncertainty_deg", "speed_uncertainty_mps", "vertical_velocity_uncertainty_mps" },
             "area": { "center_lat", "center_lon", "range_half_extent_m", "dwell_angle_half_extent_deg" },
             "platform_orientation": { "heading_deg", "pitch_deg", "roll_deg" },
             "sensor_orientation": { "heading_deg", "pitch_deg", "roll_deg" } },
  "job_id": 2,
  "job": { "job_id", "sensor_type": {code,name}, "sensor_model", "target_filtering_flag",
           "area_filtering", "area_blanking", "sector_blanking", "radar_priority", "end_of_job",
           "bounding_area": { "points": [[lat,lon] x4] }, "radar_mode": {code,name},
           "nominal_revisit_interval_s", "nominal_along_track_uncertainty_m",
           "nominal_cross_track_uncertainty_m", "nominal_alt_uncertainty_m",
           "nominal_track_heading_uncertainty_deg", "nominal_speed_uncertainty_mps",
           "nominal_slant_range_std_m", "nominal_cross_range_std_deg",
           "nominal_radial_velocity_std_mps", "nominal_mdv_mps",
           "nominal_detection_probability_pct", "nominal_false_alarm_density_neg_db",
           "terrain_elevation_model": {code,name}, "geoid_model": {code,name} },
  "mission": { "plan", "flight_plan", "platform_type", "platform_type_code", "platform_config",
               "reference_date" },
  "packet": { "version", "nationality", "classification", "classification_code",
              "classification_system", "code", "code_names", "exercise", "exercise_indicator",
              "exercise_indicator_code", "platform_id", "mission_id", "job_id" } }
```

Notes:

- `classification.probability` is D32.11 converted to a fraction from 0 to 1.
- `location_error` comes from one of two sources:
  - **The target report.** Uses D32.12-D32.15 (one standard deviation) and sets `"source": "target_report"`.
  - **The job definition.** Used when the report carries none of D32.12-D32.15, and only if the job gives at least one nominal value (para 2.7.16). Sets `"source": "job_nominal"`, with:
    - `slant_range_m` from J21;
    - `cross_range_deg` from J22 (an angle, not a distance);
    - `radial_velocity_mps` from J23.
- `ground_error`: the target's one-sigma error on the ground, see [Ground error](#ground-error).
- `lat` and `lon` are left out in two cases:
  - the report has no usable location form;
  - the high-resolution latitude is exactly -90 (SA32 `0x80000000`). Real data uses this, paired with longitude 180, as a "no location" placeholder.
- J22 values of 180 degrees or more are No Statement or out of range, so they are dropped.

### `emit: segments`

The decoder writes one record per segment:
`{ "record": "<name>", "packet": {...}, ...typed segment fields }`.

- The names are `mission`, `dwell`, `hrr`, `range_doppler`, `job_definition`,
  `free_text`, `low_reflectivity_index`, `group`, `attached_target`,
  `test_and_status`, `system_specific`, `processing_history`,
  `platform_location`, `job_request`, `job_acknowledge` and `unknown`.
- `dwell` records carry their target reports as a `targets` array.
- `dwell`, `test_and_status` and `platform_location` records also get `time` and `time_basis`.

### `emit: all`

The decoder writes both kinds of record in stream order. Each `dwell` record comes before its `target` records.

### `platform: true` (any mode)

The decoder also writes the sensor platform's own state, one `platform` record per dwell
(D6-D17) and per platform location segment (L1-L7), just before that segment's other records:

```jsonc
{ "record": "platform", "from": "dwell",           // or "platform_location"
  "time": "...", "time_basis": "...",
  "lat": -34.9, "lon": 138.2, "alt_m": 7620.0,       // metres above the WGS 84 ellipsoid
  "track_deg": 90.0, "speed_mps": 200.0, "vertical_velocity_mps": 0.0,
  "uncertainty": { "along_track_m", "cross_track_m", "alt_m", "track_deg", "speed_mps" },  // dwell only
  "platform_id": "DEAP", "mission": {...}, "packet": {...} }
```

### Warnings (every mode)

When a segment cannot be decoded, the decoder skips it and writes
`{ "record": "warning", "segment_type", "segment_name", "bytes_len", "error", "packet" }`.
This covers:

- a segment size that is too small or runs past the end of the packet;
- a body that ends early;
- a mandatory dwell field missing from the existence mask;
- an invalid HRR H25 or H26 value.

## Ground error

A target record carries `ground_error` when it has `lat`/`lon` and both a
slant range and a cross-range sigma are known. Each sigma (one standard
deviation) is taken from the first of:

1. the target report: D32.12 slant range (m) and D32.13 cross range (already a distance, m);
2. the job definition's nominal values: J21 slant range (m) and J22 cross range (an angle, degrees);
3. the decoder options `range_sigma_m` and `cross_range_sigma_deg` (an angle, degrees).

The two sigmas are chosen independently. `source` names the least specific
of the two (`target_report` < `job_nominal` < `options`). If either sigma is
still missing, `ground_error` is left out.

Geometry: first-order propagation of the polar errors into a flat-earth
tangent plane at the sensor (D7-D9). The target's east/north offset from the
sensor uses the WGS 84 meridional and prime-vertical radii of curvature at
the sensor latitude. Earth curvature is ignored, which at GMTI ranges moves
the grazing angle by well under a degree. With

- ρ = ground range, Δh = sensor altitude (D9) − target height (D32.6, else 0),
- R = √(ρ² + Δh²) the slant range, ψ = atan2(Δh, ρ) the grazing angle,
- β = bearing from sensor to target (degrees clockwise from north),

the sigmas on the ground are

- σ_g = σ_R / cos ψ = σ_R · R / ρ (range, along β);
- σ_x = R · σ_θ with σ_θ in radians, or the D32.13 distance as is (cross range, along β + 90°).

The covariance in the (range, cross-range) axes is diag(σ_g², σ_x²). Rotated
into north/east it is

- nn = σ_g² cos²β + σ_x² sin²β
- ee = σ_g² sin²β + σ_x² cos²β
- ne = (σ_g² − σ_x²) sin β cos β

The ellipse is that covariance's eigen-decomposition: `semi_major_m` and
`semi_minor_m` are the square roots of its eigenvalues, and `orientation_deg`
is the major axis in degrees clockwise from true north, in [0, 180) (0 for a
circle). The record also gives `range_sigma_m` (σ_g), `cross_range_sigma_m`
(σ_x), `slant_range_m` (R), `ground_range_m` (ρ), `bearing_deg` (β) and, when
D32.15 or J23 gives it, `radial_velocity_sigma_mps`. All values are one sigma,
in metres, square metres or degrees.

Below 1 m of ground range (the target beneath the sensor) there is no
bearing. The decoder then uses σ_g = σ_R, leaves out `bearing_deg`, and gives
a circle whose radius is the larger of σ_g and σ_x, with orientation 0. No
field is ever NaN or infinite; if an input is not finite, `ground_error` is
left out.

## Time basis

D6 (and T4, L1) counts milliseconds from 00:00 UTC on the mission reference
day, M5-M7 (para 2.4.6, Annex B para 5). The count can pass 86 400 000 on
multi-day missions.

- **After a mission segment has been seen:** `time` = reference day + ms, and
  `time_basis` = `"mission_reference_date"`.
- **Before any mission segment:** the packet header has no date, and the
  standard says the day is unresolved until a mission segment arrives. So
  the decoder:
  1. takes the count modulo one day as a UTC time of day;
  2. puts it on whichever calendar day (yesterday, today or tomorrow) lands
     closest to the decoder's UTC clock;
  3. sets `time_basis` = `"receipt_clock_time_of_day"`.

  This is correct for live feeds. Mission segments must be sent at least
  every two minutes (para 2.3), so recorded files normally start with one.

## Interpretation notes

Some parts of the standard are ambiguous. The decoder resolves them as follows:

- **Table 2-4.2** lists "143 Tagging Device" and then "143-253 Reserved". The
  decoder names both 142 and 143 "Tagging Device".
- **Table 2-5.1** gives H32.3 and H32.4 as "1 byte, I16". The decoder reads
  them as 2-byte I16 values.
- **Scatterer records** are read as whole records until the segment ends. The
  record width comes from H25, H26 and mask bits H32.1-H32.4.
- **Annex B 6.7:** the SA16 worked example's bit pattern is one LSB away from
  its stated angle.

## Tests

```sh
cargo test -p ot-codec-stanag4607
```

The tests use the real sample corpus from `$OT_GMTI_SAMPLES` (default
`$HOME/data/gmti/STANAG4607`). The samples are not in the repository. If the
directory is missing, those tests print a message and skip.

To print a summary of the corpus:

```sh
cargo test -p ot-codec-stanag4607 --test samples corpus_summary -- --nocapture
```

# Third-party notices

OpenTrack is released under the [MIT License](LICENSE). The repository also contains the
third-party components and data below, and each keeps its own license. Dependencies fetched at build
time (Rust crates and npm packages) carry their own license files. `cargo deny check licenses`
(see `deny.toml`) lists the licenses allowed for Rust dependencies.

## Code vendored in this repository

| Component | Where | License | Notes |
|---|---|---|---|
| [rumqttc](https://github.com/bytebeamio/rumqtt) | `third_party/rumqttc/` | Apache-2.0 ([LICENSE-APACHE](third_party/rumqttc/LICENSE-APACHE)) | Modified. The changes are listed in `third_party/rumqttc/OPENTRACK.md`. |
| [samael](https://github.com/njaremko/samael) | `third_party/samael/` | MIT ([LICENSE](third_party/samael/LICENSE)) | Modified. The changes are listed in `third_party/samael/OPENTRACK.md`. |

## Data and assets

| Component | Where | License | Source |
|---|---|---|---|
| MIL-STD-2525C to 2525D crosswalk | `ui/src/lib/milsym/crosswalk.json` | Apache-2.0 | Derived from Esri [joint-military-symbology-xml](https://github.com/Esri/joint-military-symbology-xml) (JMSML), `samples/legacy_support/LegacyMappingTableCtoD.csv`, by `scripts/vendor-symbol-crosswalk.py` at a pinned commit. Copyright Esri. |
| Noto Sans Medium map glyphs | `ui/public/map-fonts/` | SIL Open Font License 1.1 ([OFL.txt](ui/public/map-fonts/OFL.txt)) | [protomaps/basemaps-assets](https://github.com/protomaps/basemaps-assets), by `scripts/vendor-map-glyphs.sh`. |
| World country outlines (1:110m) | `ui/public/world-110m.geo.json` | Public domain | [Natural Earth](https://www.naturalearthdata.com/) |
| Common-password list | `crates/ot-server/src/auth/common-passwords.txt` | MIT | [SecLists](https://github.com/danielmiessler/SecLists), Copyright (c) 2018 Daniel Miessler |
| Autoferry sensor-fusion test excerpts | `crates/ot-server/tests/data/autoferry/` | CC0 1.0 | NTNU Autoferry sensor fusion benchmark dataset (see the folder's README) |

## Runtime npm packages of note

| Package | License | Used for |
|---|---|---|
| [@kineticlogic/staresdk](https://github.com/kineticlogic-io/stareSDK) | MIT | The UI components, theme and design tokens (same publisher) |
| [milsymbol](https://github.com/spatialillusions/milsymbol) | MIT | Drawing MIL-STD-2525 symbols |
| [mil-std-2525](https://www.npmjs.com/package/mil-std-2525) | MIT | The MIL-STD-2525D symbol catalog in the symbol designer |
| [MapLibre GL JS](https://github.com/maplibre/maplibre-gl-js) | BSD-3-Clause | The track map |

## Standards

OpenTrack implements several published interface standards: STANAG 4607 (GMTI), SAPIENT (BSI Flex
335), Cursor-on-Target, and field names from the OTH-GOLD track database model. These are
independent implementations, and the repository does not reproduce the standards' text. To get a
standard, go to its publisher. STANAG 4607 is available from the
[NSG Standards Registry](https://nsgreg.nga.mil/doc/view?i=5568) (see `crates/ot-codec-stanag4607/README.md`).
SAPIENT message definitions come from the [`sapient-rs`](https://crates.io/crates/sapient-rs)
crate (MIT OR Apache-2.0).

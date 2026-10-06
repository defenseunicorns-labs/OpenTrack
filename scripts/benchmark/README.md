# Benchmark

Scores OpenTrack's tracker stages and correlation against truth, so a change to an algorithm or
a setting can be measured rather than eyeballed on the map.

```sh
scripts/benchmark/setup.sh                  # once: a venv with numpy and scipy (.venv, not committed)
scripts/benchmark/bench build all           # fetch the data and build every scenario
scripts/benchmark/bench run all --label x   # run them through OpenTrack and score
scripts/benchmark/bench compare RUN_A RUN_B # two runs side by side, each change marked better or worse
```

`run` builds `target/release/opentrack` (skip with `--no-build`) and starts a throwaway Redis
in Docker (or use `--redis URL`). `--correlation settings.json` runs every scenario with other
correlation settings, for sweeps. Names take a trailing `*` (`bench run 'autoferry-*'`).

## Data stays out of the repository

Everything the benchmark downloads, builds and writes lives under `$OT_BENCH_DATA` (default
`~/data/benchmark`):

```
raw/          downloads (Autoferry, Stone Soup demo data)
scenarios/    built scenarios: scenario.json, one frames file per source, truth.jsonl
results/<run>/  per scenario: tracks.jsonl, source_tracks.jsonl, plots.jsonl, trace.jsonl,
              run.json, score.json; and summary.md / summary.json for the run
```

The repository holds only the code, and the source specs the scenarios are built from (`specs/`).
The GMTI recordings and exercise GPS are local: `OT_BENCH_GMTI` and `OT_BENCH_GPS` (default
`~/data/gmti/STANAG4607` and `~/data/gps-all`).

## How a run works

`opentrack bench <scenario> --out <dir>` loads the scenario's source specs into a database of its
own and plays every source's frames, in time order, through that source's real pipeline: codec,
mapping and tracker stage. The observations then go through the real engine, one second of
scenario time at a time. Lifecycle and timers follow the scenario's clock, not the wall clock, so
an hour of data runs in seconds or a few minutes. Nothing is published: no writer runs, and the
engine works in a Redis namespace of its own, removed afterwards.

It writes:

- the system tracks at every sample time
- every observation the pipelines produced (the tracker stages' tracks)
- every plot a tracker stage was given
- the engine's trace, which `replay/replay-video.py` renders as a video

## Scenarios

| Scenario | Data | Sources | Truth |
|---|---|---|---|
| `autoferry-N` (N = 2, 3, 4, 5, 6, 13, 16, 17, 22) | Autoferry, NTNU, [CC0](https://github.com/Autoferry/sensor_fusion_dataset): recorded lidar and radar from a ferry in Trondheim | lidar and radar plots, each through its tracker stage (as `docs/examples/autoferry`) | the reference vessels' GNSS |
| `autoferry-N-fusion` | the same | plus a vessel track feed made from the truth (every 3 s, 2 m noise) | the same |
| `solent-radar` | Stone Soup demo data: Solent AIS, 85 min, 87 vessels | a simulated coastal radar at Southsea (3 s revisit, 24 km, 0.5°, Pd 0.85, 10 false plots a scan) | the AIS, interpolated |
| `solent-fusion` | the same | plus the AIS as a track feed (MMSI identifiers) | the same |
| `opensky-radar` | Stone Soup demo data: OpenSky ADS-B over England, 20 min, 83 aircraft | simulated air surveillance radars at Heathrow and Manchester (the demo's sites: 4 s, 110 km, 0.15°, Pd 0.9) | the ADS-B, interpolated |
| `opensky-fusion` | the same | plus the ADS-B as a track feed (ICAO identifiers) | the same |
| `synthetic-crossing` | generated | two radars, 12 targets crossing near one point (some turning), 40 false plots a scan, Pd 0.8 | the generated paths |
| `acoustic-sparse`, `acoustic-demo`, `acoustic-dense` | generated | 3, 5 and 8 fixed acoustic arrays reporting bearing (+ often range) to 2, 6 and 14 small UAS (`docs/non-point-contacts.md`, "Acoustic arrays"; see `acoustic.py`) | the generated paths |
| `gmti-garden-island-1013`, `-1014` | local: STANAG 4607 over Garden Island, Adelaide, 13 and 14 Oct 2015 | the GMTI through the STANAG 4607 codec and the GNN tracker | the exercise GPS logs |
| `gmti-…-fusion` | the same | plus the GPS as a track feed | the same |

The radar model follows Stone Soup's radar on a fixed platform (`radar.py`):

- every target in coverage is detected with probability Pd
- range and bearing noise is Gaussian
- clutter is Poisson and uniform over the coverage
- each plot carries its error ellipse

Truth points that no sensor covers are don't-care: they count neither as missed nor as false.

The GMTI recordings are real and did not see everything. A GPS point only counts while the vehicle
moves faster than the radar's minimum detectable velocity, inside a dwell footprint, with a plot
within 150 m. Tracks away from every GPS vehicle are reported as unlabelled (other traffic), not
false.

## Measures

At every sample time, the confirmed system tracks are compared with the truth. Tracks are
dead-reckoned from their last report, as a display would draw them. The results are reported for
the system tracks and for each tracker stage on its own. See `scoring.py` for the exact
definitions.

| Measure | Meaning |
|---|---|
| **GOSPA** (p = 1, α = 2, cutoff per scenario) | Stone Soup's GOSPAMetric: the optimal assignment of tracks to truths. It splits into localisation, missed (c/2 per truth left over) and false (c/2 per track left over). The mean per sample; lower is better. |
| **complete** | Share of truth samples with a track. |
| **spurious** | Share of tracks near no truth (only when the truth is complete). |
| **ambiguity** | Tracks within the cutoff of each tracked truth (1 is ideal). |
| **pos RMS / velocity RMS** | Error of the assigned tracks. |
| **ID chg/h** | Track number changes per truth hour. |
| **frag** | Distinct tracks per truth. |
| **longest** | Share of a truth's time on the one track that held it longest. |
| **time to first track** | Median, from a truth's first scored sample. |
| **pairing P / R** | Correlation: of the source-track pairs the engine put on one system track, how many are the same object (precision); of the pairs that should have been joined, how many were (recall). |

A source track's truth comes from:

- its key, when a feed reports by the truth id
- otherwise, the truth four in five of its reports were nearest to

## Adding a scenario

1. Write a builder in `scenarios/`. It returns `SCENARIOS = {name: build_function}`, and each
   build uses `common.Writer`:
   - `frame(source, t, json_=… | text=… | hex_=…)` for each frame as the transport would deliver it
   - `truth_point(t, id, lat, lon, alt, score=…)` for each true position
   - `finish(description, source_specs, engine=…, notes=…)` at the end
2. Add it to `scenarios/__init__.py`.

`notes` sets how it is scored:

- `gospa_cutoff_m`
- `truth_complete`
- `detected_within_m`: for real data the sensor only partly saw

Sensor profiles are a natural next step: a directory of tracker configurations per sensor (see
the todo list).

## Replay tools

`replay/` holds the tools that feed a running OpenTrack in real time, for demos and for watching
on the map. The benchmark does not use them.

- `gmti-rebroadcast.py`: STANAG 4607 recordings over TCP
- `gmti-gps-replay.py`: GMTI and exercise GPS on one clock
- `autoferry.py`: builds the Autoferry test fixtures for the engine's `replay_autoferry_*` tests
- `autoferry-replay.py`
- `replay-video.py`: renders an engine trace
- `esm-video.py`: renders the `esm-crossfix` scenario (an overview, ghost rules off against on, one ship followed) and the single-sensor `esm-patrol` scenario
- `acoustic-video.py`: renders an `acoustic-sparse`/`acoustic-demo`/`acoustic-dense` run (arrays, bearing lines, drones and the tracks the correlator makes), with a live count of tracks fused across arrays against tracks still a single array's fragment

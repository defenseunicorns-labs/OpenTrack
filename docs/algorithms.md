# Algorithm versions

OpenTrack versions the behaviour of its correlation engine and of each tracker
separately from the release version, because they are what change while we tune
against recorded data. A version names *what the algorithm does*: bump it whenever
the same inputs would give different output, add an entry below, and record its
scores on the benchmark.

| Component | Constant | Stamped on |
|-----------|----------|------------|
| Correlation engine: pairing and merging, best source, detection association, lifecycle, publish rule | `correlate::VERSION` (ot-server) | every engine decision in the track graph (`evidence.correlation_version`), every published message (`publisher.correlation`), `/api/v1/status` |
| GNN tracker | `tracker::GNN_VERSION` (ot-source) | every track report it makes (`provenance.tracker`), kinematic merge evidence (`trackers`), `/api/v1/status` |
| MHT tracker | `tracker::MHT_VERSION` (ot-source) | same as GNN |
| Tracker and scorer plugins | the plugin's manifest `version` | `provenance.tracker` as `<plugin>-<version>`; a scorer's in kinematic merge evidence (`scorer.plugin`) |

Plugins version themselves ([plugins.md](plugins.md)): a plugin changes what it outputs, it
bumps its manifest version; the benchmark (`bench run --plugin`) scores it against the built-in
trackers the same way.

A test (`algorithm_versions_are_documented`) fails if a running version has no entry
here, so a bump cannot ship without its notes.

## Benchmark

Autoferry sensor fusion dataset (NTNU, CC0), scenarios 2 (crossing) and 16 (longest
range), replayed by the tests in `crates/ot-source/src/tracker/tests.rs` and
`crates/ot-server/src/engine.rs` (`replay_autoferry_*`). Run them with
`OT_TEST_REDIS_URL=redis://127.0.0.1:6379 cargo test autoferry -- --nocapture`. The wider benchmark
(`scripts/benchmark/`, GOSPA and SIAP-style scores over nine Autoferry scenarios, GMTI, AIS and
ADS-B with simulated radars) is how parameter and algorithm changes are compared.

- **Tracker**: per vessel, the reports and purity of its best track; tracks that mostly
  follow clutter.
- **Correlation, sensor tracks**: lidar and radar tracks (from the tracker, or from the
  fixtures' own simple tracker) paired with the vessel track feed: right, wrong,
  unpaired, and clutter tracks paired with a vessel.
- **Correlation, raw detections**: plots associated with the right vessel's track, wrong,
  missed (mostly a second plot on a vessel in the same scan), clutter associated.

### GMTI and tracker auto timing

A tracker's `auto_timing` sets its windows from the sensor's revisit period, which the
source's codec measures from the stream (STANAG 4607: the time between the first dwells
of consecutive revisits of a job, a running median; the job's nominal revisit until one
revisit is complete). Each window is a multiple of the period, never below a floor:

| Window | Multiple | Floor |
|--------|----------|-------|
| drop an unconfirmed track without a plot | 1.3 revisits | 8 s |
| drop a confirmed track without a plot | 2.5 revisits | 30 s |

With the revisit known, a track's existence (gnn-2, mht-2) counts a miss once per revisit
without a plot, not once per scan: a GMTI scan is one dwell, which may not have looked at
the track. (Before gnn-2 auto timing also set the confirmation window, 3.5 revisits and at
least 20 s; confirmation is now by existence.)

The floors cover what the revisit does not: a ground mover drops out of GMTI for longer
than a fast revisit (terrain, stopping below the minimum detectable velocity). The
windows follow a new job's period; changes within 5% are ignored. It is a setting, not a
new tracker version: the same windows give the same output as before.

Scored on real GMTI recordings (not in the repository; `$OT_GMTI_SAMPLES`), by the share
of GNN tracks that last 30 s or more (fewer, longer tracks mean less fragmentation):

| Recording | Revisit (measured; nominal says 6 s) | Pure multiples | Auto timing (with floors) |
|-----------|---------|----------------|---------------------------|
| Stuart Hwy 48 | 2.56 s (7 dwells) | 39 of 100 | 79 of 85 |
| Stuart Hwy 94 | 2.56 s | 26 of 88 | 98 of 107 |
| Stuart Hwy 115 | 12.8 s (25 dwells) | 25 of 29 | 25 of 29 |

`crates/ot-source/tests/stanag4607.rs` runs the example source over the first and last.
With gnn-2 (existence; the example's detection probability 0.5, clutter 3·10⁻⁸ /m², birth
3·10⁻⁹ /m², and a ground error ellipse per detection from the radar geometry): 72 of 79,
83 of 93 and 19 of 22.

## Changelog

### correlation-6 (2026-10-02)

Non-point contacts: the known limits of `esm-crossfix` (#27), and acoustic arrays. Point-only feeds
are unchanged: every change is on the bearing path, or on areas over 2 km of error carrying an
evidence identifier (an ELNOT). The Autoferry replays score as before (0 wrong pairings).

- **An area pairs on its emitter identity.** An area whose ELNOT a track carries (on its identifiers
  or its bearings), and which fits that track at least 10 times better than any other in its gate,
  the ELNOT counting 10:1, pairs with it on the second such report (`rule: area-emitter`).
- **A fix track joins the track its lines went to**, once every sensor track in it reports for that
  track (`rule: fix-lines`).
- **A fix never holds two tracks of one sensor.**
- **A bearing goes to the track its sensor track's own ranged reports are on**, before any other rule,
  and a cross-fix whose line belongs to another track that way cannot bind to this one.

| Scenario | `correlation-5` | `correlation-6` |
|---|---|---|
| esm-crossfix: ships with two tracks, wrong bearings, ghost fixes | 3, 26, 3 | 1, 0, 0 |
| esm-crossfix with ELINT on AIS ships: ships with two tracks; areas on their AIS ship's track | 7; 0 | 2; 52 of 100 |
| esm-patrol: ELINT joins the boat's track | 460 s | 370 s |
| acoustic-arrays: ranged reports on a track with another drone; bearings likewise | 1,596; 598 | 0; 0 |
| Areas paired with another object, every scenario | 0 | 0 |

Details and the cases left open: [non-point-contacts.md](non-point-contacts.md).

### correlation-5 (2026-09-26)

Fewer wrong pairings in crowded harbours. The benchmark's harbour gate is wrong pairings pooled
over the Autoferry and Solent fusion scenarios. It was 5.4%, against a target under 2%. Two causes,
from the comparison trace:

- **Splits could not happen against a slow feed.** A split needs `m` of the last `n` comparisons
  (5 of 6) outside its gate. They were counted only when both sides had a new report, and within
  the pairing window (30 s). Against AIS every 10 s, at most three fell in the window, so a radar
  track that followed another vessel stayed on the wrong track indefinitely. It was 500 m away for
  nine minutes in one case.
  - The split check now has its own window, `split.window_secs` (300 s).
  - It may reuse the other side's last report, weighted as pairing does.
- **A close approach counted the same in a harbour as at sea.** "Another object nearby" had a fixed
  density, `object_density_per_km2` (1 per km²).
  - The engine now counts the live tracks within `local_density_radius_m` (500 m) of the report and
    uses their density when it is higher.
  - Among moored vessels a radar track now needs more comparisons before it pairs.

Also tried and dropped: requiring the best candidate to lead the next one by a margin, which did not
help. Splitting after fewer misses did not help either (3 of 4 or 4 of 5): the extra splits paired
again, wrongly.

Benchmark against correlation-4, with splits proposed (the default):

| Scenario | GOSPA | Pairing precision / recall | ID changes / truth hour | Fragmentation | Fused position RMS |
|---|---|---|---|---|---|
| solent-fusion | 7,151 → 9,805 | 95.6% / 78.2% → 98.8% / 73.2% | 89 → 59 | 14.5 → 8.5 | 44 → 35 m |
| opensky-fusion | 6,862 → 7,070 | 100% / 90.4% → 100% / 89.4% | 12.5 → 12.7 | 1.51 → 1.51 | 146 → 142 m |
| synthetic-crossing | 655 → 663 | 99.0% / 93.1% → 100% / 92.7% | 65 → 63 | 8.9 → 9.3 | 54 → 54 m |

- **Harbour gate:** 5.36% → 3.21% wrong. With `split.automatic` on, it is 2.00%, recall 78.1%.
- **Automatic splits are now the default** (`split.automatic: true`; `propose` still offers them).
  A sensor track that stopped following its object leaves on its own. Judged at each moment, it is
  the difference between 25.6% and 8.7% wrong pairings over the harbour scenarios.
- **The Autoferry lidar now uses MHT, not GNN.** Two boats passing within metres swapped under GNN.
  With this, the harbour gate is 1.51% (1.70% with splits only proposed).
- **The cost is GOSPA's false-track term.** Radar and AIS tracks of a crowded harbour now stay apart
  more often, as two tracks, rather than pairing wrongly.

### correlation-4 (2026-09-26)

Radar tracks now join their AIS or ADS-B track. The benchmark found about half of them never
paired (correlation recall of about 50% on `solent-fusion` and `opensky-fusion`). The engine's
comparison trace showed three causes:

- **A fresh report can reuse a held view.** A comparison used to count only when *both* sides had
  a new report. A moored vessel's AIS arrives every few minutes, so a radar track (median life
  about 3 min) never collected its three comparisons.
  - A new report may now be compared with the other side's view already used. Its evidence is
    weighted by the new report's share of the combined position variance: a precise AIS position
    reused against noisy radar counts nearly in full, and two equally noisy views count half.
  - A reused comparison counts no sooner than `reuse_interval_secs` (10 s) after the pair's last
    one. A tracker smooths its output, so its consecutive reports are not independent. Reuse every
    3 s cost 14 points of precision among the moored vessels of a harbour; at 10 s it costs 2.
    Switch reuse off with `reuse_views: false`.
- **Stopped targets drift.** A view that reports no motion (speed within `speed_sigma_mps`) is
  propagated as a slow drift, `stopped_drift_mps` (0.5 m/s) times the time elapsed. It no longer
  gets the white-acceleration growth, which put a moored vessel's three-minute-old position at
  2 to 5 km of uncertainty, so no comparison against it could decide anything. Views may now be
  180 s old (`max_age_secs`, was 30).
- **Aircraft get their own velocity spread.** "Another object nearby" had a 15 m/s velocity spread,
  a surface-traffic figure. When either report is an aircraft it is now `air_velocity_spread_mps`
  (60 m/s).
- **The same-source rule has its own setting.** How long a source's track counts as live on a system
  track, for the rule that a sensor's two tracks are two objects, was `max_age_secs`. It is now
  `source_live_secs` (30 s), so older views can be compared without keeping stale tracks in the
  way.

Benchmark: `scripts/benchmark`, baseline against `correlation-4`:

| Scenario | GOSPA | Pairing precision / recall | ID changes / truth hour | Fused position RMS |
|---|---|---|---|---|
| solent-fusion (AIS + coastal radar) | 11,869 → 7,151 | 98% / 49% → 96% / 78% | 62 → 89 | 20 → 44 m |
| opensky-fusion (ADS-B + two radars) | 17,866 → 6,862 | 100% / 50% → 100% / 90% | 64 → 12.5 | 121 → 146 m |
| autoferry, all 9 fusion scenarios | unchanged | unchanged | unchanged | unchanged |

The costs on `solent-fusion` are the 4% of wrong pairings, which join the radar tracks of
neighbouring moored vessels (the position RMS), and more distinct system tracks per vessel while
radar tracks join (ID changes and fragmentation). The next step there is the radar tracker's own
fragmentation: 23.6 tracks per vessel on `solent-radar`.

In a benchmark run the engine now also writes every pairing comparison to its trace (`compare`:
the candidate, why it was skipped or its distance, σ, d², ln LR and probability). That trace is how
these causes were found.

### correlation-3 (2026-09-25)

Proper error propagation and a probability for every pairing:

- **Comparison.** Each report becomes a state with a covariance: its own full covariance
  (a tracker's), else its ellipse or circular error (floored at `min_sigma_m`), and its
  course and speed with `speed_sigma_mps` (a speed within that and no course, as a
  stopped GPS reports, is a velocity of zero; a domain-wide spread when it reports none).
  The older report is propagated to the newer one's time (constant velocity,
  white-acceleration noise `process_noise_mps2`) and their difference tested against the
  sum of both covariances: position and velocity (4 degrees of freedom) when both report
  motion, else position. The two are taken as independent (separate sensors); the common
  process noise of two tracks of one object is not modelled. Replaces the circular σ and
  flat drift of correlation-2, which never compared velocity.
- **Evidence.** A comparison is a likelihood ratio: the Gaussian density of the difference
  against another object being there (`object_density_per_km2`, and
  `velocity_spread_mps` for velocity). Outside the gate (`gate_probability`, 0.99) it
  counts at most (1 − 0.99) : 1. A comparison counts only when both sides bring a report
  newer than the one each gave the last counted comparison (new reports against one side's
  same old report are not new evidence), at least `min_interval_secs` apart (consecutive
  reports are not independent either); the last `n` within the window count.
- **Pairing.** The probability of the same object is `prior_probability` (0.01) updated
  by that evidence; pair (or suggest) at `pair_probability` (0.99) after at least `m`
  comparisons. Replaces m of n passing a chi-square gate.
- **Confidence.** A paired source track's probability of being the same object starts at
  the pairing threshold and follows its comparisons with the rest of the track; it is its
  `confidence` on the track (was always 1). A split needs it at `split_probability`
  (0.001) and `m` of the last `n` comparisons outside a 0.9999 gate (a constant-velocity
  model understates a manoeuvring target's error). A track's confidence, the `confidence`
  built-in of the output schema, is `1 − Π(1 − existence · confidence)` over its source
  tracks (existence: the source's own `provenance.confidence`, 1 when it states none).
- **View.** A view whose position comes from a report without motion (a plot) takes course
  and speed from the newest report in the window that has them, so it can be propagated.
- Settings saved before (`chi2_gate`, `drift_mps`, the split `chi2_gate`) are still read:
  a pairing chi-square gate becomes the equivalent gate probability.

Autoferry scores are unchanged from correlation-2 (same tables, and no split suggestions),
with gnn-2 and mht-2 tracks paired as gnn-1 and mht-1 tracks were. On the GMTI and GPS
replay of 13 Oct 2015 (Garden Island), correlation-2 paired a GMTI track passing a parked
GPS vehicle at 56 m (speeds 6 m/s and 0) after four position comparisons; correlation-3
does not pair them (`a_moving_track_does_not_pair_with_a_still_one_nearby`). The first
correlation-3 build still paired the same vehicle with a GMTI track that had stopped
reporting, by comparing five new GPS fixes with its one last report; hence the rule that
both sides bring new reports.

### gnn-2 (2026-09-25)

Every track carries its probability of being a real target, and the probability decides
(mht-2 is the same change on the MHT, whose leaves each carry their own):

- **Existence** (IPDA with the assigned plot): a new track starts at the prior
  `birth / (birth + clutter)`; between looks it decays with `target_lifetime_secs`; a plot
  multiplies its odds by `Λ + 1 − Pd`, where `Λ = Pd · g / λ` (the plot's Gaussian
  likelihood against the clutter density); a look without one by `1 − Pd`. With a known
  revisit (auto timing) a miss counts once per revisit, else once per scan.
- **Decisions**: confirmed at `confirm_probability` (0.95, and at least `confirm_hits`
  plots); dropped at `drop_probability` (0.02) or after the drop time without a plot.
  Replaces 3 plots in 5 s (`confirm_within_secs` is still read, no longer used).
- **Measurement error**: each plot's own covariance (its ellipse, else its circular
  error, else `measurement_sigma_m`) instead of a circle.
- **Reports** carry the existence as `provenance.confidence`, the position ellipse, and
  the full position and velocity covariance (`uncertainty.covariance`).
- The detection model (`detection_probability`, `clutter_density`, `birth_density`) is a
  stage setting for both algorithms; the MHT-only settings from before still apply when
  set.

| Scenario | Sensor | GNN best track per vessel | Clutter tracks | MHT best track per vessel | Clutter tracks |
|----------|--------|---------------------------|---------------:|---------------------------|---------------:|
| 2 | lidar (cluster 10 m, Pd 0.8) | 186, 99%; 133, 100% | 0 | 185, 99%; 132, 100% | 0 |
| 2 | radar (clutter 4·10⁻⁶) | 63, 100%; 68, 100% | 18 | 63, 100%; 68, 100% | 14 |
| 16 | lidar | 133, 100%; 114, 100% | 0 | 133, 100%; 115, 100% | 0 |
| 16 | radar | 67, 100%; 62, 100% | 5 | 59, 100%; 60, 98% | 4 |

(gnn-1: 1 and 23 clutter tracks in scenario 2, 1 and 4 in 16.)

### mht-2 (2026-09-25)

The gnn-2 existence model on the MHT: every leaf's history carries its existence, a
chosen leaf reports it as `provenance.confidence`, and it confirms and drops the
hypothesis's track (tree scores still decide which hypothesis is best and which trees
are deleted). Scores are in the gnn-2 table: the same as mht-1 but for one clutter track
more in scenario 16 radar (4, mht-1 3) and the same elsewhere.

### correlation-2 (2026-09-25)

Operators steer correlation, and settings change at runtime:

- **Settings** (approach, mode, gates, M of N, splits) are saved from the Correlation page
  and reloaded by the running engine; the `--correlation` flag only sets the approach until
  some are saved.
- **Suggest mode**: kinematic pairings are proposed instead of made; an operator accepts
  (merge) or rejects (the two never pair). Shared identifiers still pair on their own.
- **Do not pair**: an operator's rejection, split or explicit decision is a DO_NOT_PAIR
  edge between source tracks, checked before every pairing, identifier matches included.
- **Splits**: a source track that disagrees with the rest of its system track (chi-square
  18.4, 5 of the last 6 comparisons) is proposed for a split (or split, with automatic
  splits on). Of two, the one that may not stand alone leaves. A split puts it on a track
  of its own, leaves the rest showing only what they report (identity included), and
  records that the two do not pair again.
- Operators can also split, merge and mark "do not pair" by hand; every decision records
  its actor and the correlation version.

Autoferry scores are unchanged from correlation-1 (same tables), and neither scenario
raises a split suggestion: the crossing in scenario 2 does not make correlated tracks
disagree.

### correlation-1 (2026-09-25)

Identifier pairing behind a kinematic sanity gate; kinematic pairing (chi-square gate,
4 of 5 comparisons within 30 s, vetoed by conflicting identifiers or domains with
`kinematics-metadata`); a sensor's concurrent tracks never pair; merges keep the
published track, then the older; best source per field group; sticky identity;
detection association (nearest first, one plot per track per scan); sources end their
tracks with `state: dropped`; only tracks a stand-alone source reports for are published.

| Scenario | Input | Paired right | Wrong | Unpaired | Clutter paired |
|----------|-------|-------------:|------:|---------:|---------------:|
| 2 | fixture tracks | 4 | 0 | 0 (2 same-sensor duplicates kept apart) | 0 |
| 2 | gnn-1 tracks | 5 | 0 | 0 | 0 |
| 2 | mht-1 tracks | 5 | 0 | 0 | 0 |
| 16 | fixture tracks | 4 | 0 | 0 | 0 |
| 16 | gnn-1 tracks | 4 | 0 | 0 | 0 |
| 16 | mht-1 tracks | 4 | 0 | 0 (1 same-sensor duplicate kept apart) | 0 |

| Scenario | Raw detections to the right track | Wrong | Missed | Clutter associated |
|----------|-------------------------------:|------:|-------:|-------------------:|
| 2 | 457 / 531 | 0 | 74 (64 a second plot that scan) | 0 / 318 |
| 16 | 383 / 407 | 0 | 24 (21 a second plot that scan) | 0 / 150 |

### gnn-1 (2026-09-25)

Constant-velocity Kalman filter per track in its local frame; least-cost assignment per
scan (Hungarian method, cost = Mahalanobis distance + ln|S|); confirm after 3 plots in
5 s; drop after 3 s (tentative) or 8 s (confirmed) without one, reported as
`state: dropped`; optional plot clustering; classification at most unknown affiliation
and a domain.

| Scenario | Sensor | Best track per vessel (reports, purity) | Clutter tracks |
|----------|--------|-----------------------------------------|---------------:|
| 2 | lidar (cluster 10 m) | 185, 99%; 133, 100% | 1 |
| 2 | radar | 63, 100%; 68, 100% | 23 |
| 16 | lidar (cluster 10 m) | 133, 100%; 115, 100% | 1 |
| 16 | radar | 68, 100%; 62, 100% | 4 |

### mht-1 (2026-09-25)

Track-oriented MHT on the same filter and lifecycle: log-likelihood-ratio scores,
the best global hypothesis solved exactly per conflict cluster (branch and bound),
N-scan pruning (N = 3), duplicate-tree removal, track numbers that carry over when the
hypothesis moves between trees.

| Scenario | Sensor | Best track per vessel (reports, purity) | Clutter tracks |
|----------|--------|-----------------------------------------|---------------:|
| 2 | lidar (cluster 10 m) | 185, 99%; 132, 100% | 0 |
| 2 | radar | 63, 100%; 68, 100% | 14 |
| 16 | lidar (cluster 10 m) | 133, 100%; 115, 100% | 0 |
| 16 | radar | 59, 100%; 60, 98% | 3 |

On all nine Autoferry scenarios (Python spike, same models): MHT had 15–25% fewer false
tracks than GNN with both sensors fused, similar accuracy, about 10× the compute.

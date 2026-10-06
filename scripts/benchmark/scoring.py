"""Scores a benchmark run against the scenario's truth.

Every sample time T (the run's `sample_secs`), the tracks being scored are
compared with where the truth objects are at T:

- GOSPA (Rahmathullah, Garcia-Fernandez and Svensson, 2017), as Stone Soup's
  GOSPAMetric computes it with p = 1 and alpha = 2: an optimal assignment of
  tracks to truths with cutoff c; the distance splits into localisation
  (assigned pairs), missed (truths left over, c/2 each) and false (tracks
  left over, c/2 each). Reported as the mean over T.
- SIAP-style measures (Votruba 2001, as in Stone Soup's SIAPMetrics), with a
  truth and a track associated at T when the GOSPA assignment pairs them:
  completeness (truths with a track), ambiguity (tracks within c of each
  tracked truth), spuriousness (tracks near no truth), position and velocity
  accuracy (RMS), and identity: track number changes per truth hour,
  fragmentation (distinct tracks per truth), longest segment (the share of a
  truth's time held by its longest-held track) and time to first track.
- Correlation, where feeds and sensors report the same objects: at each T,
  every pair of source tracks the engine put on one system track is right
  (same truth) or wrong, and every pair of source tracks of one truth left on
  different system tracks is a missed pairing.

Tracks are dead-reckoned from their last report to T (course and speed, up
to MAX_EXTRAPOLATE_S), as a consumer drawing them would. Truth points marked
`score: false` are don't-care: tracks on them are neither false nor scored.
When the truth is incomplete (`truth_complete: false`: the sensors see
traffic nobody logged), tracks near no truth are counted as unlabelled
instead of false, and spuriousness is not reported. With
`detected_within_m` (real recordings where the sensor saw only some of the
truth), a truth point also counts only when a plot fell within that distance
of it within `detected_window_s`: the tracker is scored on what the sensor
gave it, not on what the sensor missed.
"""

import bisect
import json
import math
from collections import Counter, defaultdict

import numpy as np
from scipy.optimize import linear_sum_assignment

from common import Local, ts

MAX_EXTRAPOLATE_S = 60.0
SOURCE_STALE_S = 20.0


def jsonl(path):
    with open(path) as f:
        return [json.loads(line) for line in f if line.strip()]


class Truth:
    def __init__(self, points, max_gap):
        self.series = defaultdict(list)
        for p in points:
            self.series[p["id"]].append((ts(p["t"]), p["lat"], p["lon"], p.get("score", True)))
        for s in self.series.values():
            s.sort()
        self.times = {k: [p[0] for p in s] for k, s in self.series.items()}
        self.max_gap = max_gap
        lat = [p[1] for s in self.series.values() for p in s]
        lon = [p[2] for s in self.series.values() for p in s]
        self.local = Local(float(np.mean(lat)), float(np.mean(lon)))

    def require_detection(self, plots, within_m, window_s):
        """Mark points no plot came near as don't-care; how many remain scored."""
        pts = sorted((ts(p["t"]),) + self.local.en(p["lat"], p["lon"]) for p in plots)
        pt = np.array([p[0] for p in pts])
        pe = np.array([p[1] for p in pts])
        pn = np.array([p[2] for p in pts])
        kept = 0
        for k, s in self.series.items():
            for n, (t, lat, lon, scored) in enumerate(s):
                if not scored:
                    continue
                i, j = np.searchsorted(pt, t - window_s), np.searchsorted(pt, t + window_s)
                e, nn = self.local.en(lat, lon)
                seen = bool(j > i and (np.hypot(pe[i:j] - e, pn[i:j] - nn) <= within_m).any())
                s[n] = (t, lat, lon, seen)
                kept += int(seen)
        return kept

    def at(self, t):
        """[(id, e, n, ve, vn, scored)] at t."""
        out = []
        for k, s in self.series.items():
            times = self.times[k]
            i = bisect.bisect_right(times, t)
            if i == 0 or i > len(s):
                continue
            if i == len(s):
                if abs(times[-1] - t) > 1e-6:
                    continue
                i -= 1
                a = b = s[i]
            else:
                a, b = s[i - 1], s[i]
            if b[0] - a[0] > self.max_gap:
                continue
            f = (t - a[0]) / (b[0] - a[0]) if b[0] > a[0] else 0.0
            ea, na = self.local.en(a[1], a[2])
            eb, nb = self.local.en(b[1], b[2])
            dt = b[0] - a[0]
            ve, vn = ((eb - ea) / dt, (nb - na) / dt) if dt > 0 else (0.0, 0.0)
            out.append((k, ea + f * (eb - ea), na + f * (nb - na), ve, vn, a[3] and b[3]))
        return out


def dead_reckon(local, lat, lon, course, speed, dt):
    e, n = local.en(lat, lon)
    ve = vn = None
    if course is not None and speed is not None:
        r = math.radians(course)
        ve, vn = speed * math.sin(r), speed * math.cos(r)
        dt = max(0.0, min(dt, MAX_EXTRAPOLATE_S))
        e, n = e + ve * dt, n + vn * dt
    return e, n, ve, vn


class Scorer:
    """Accumulates the measures over sample times for one set of tracks."""

    def __init__(self, cutoff, truth_complete):
        self.c = cutoff
        self.complete = truth_complete
        self.samples = 0
        self.gospa = []
        self.loc, self.miss, self.false = [], [], []
        self.truths = self.tracked = 0
        self.near_counts = []  # tracks within c of each tracked truth
        self.tracks_total = self.spurious = self.unlabelled = 0
        self.pos_err, self.vel_err = [], []
        self.history = defaultdict(list)  # truth id -> [(t, uid or None)]
        self.uids = set()
        self.live = []
        # Identity: samples each (truth, track) spent within the cutoff, and
        # each truth's and track's samples, for IDF1.
        self.co = defaultdict(int)
        self.truth_samples = self.track_samples = 0

    def add(self, t, truths, tracks):
        """truths: [(id, e, n, ve, vn, scored)]; tracks: [(uid, e, n, ve, vn)]."""
        self.samples += 1
        self.uids.update(x[0] for x in tracks)
        self.live.append(len(tracks))
        c = self.c
        if truths and tracks:
            te = np.array([[x[1], x[2]] for x in truths])
            tr = np.array([[x[1], x[2]] for x in tracks])
            d = np.hypot(te[:, None, 0] - tr[None, :, 0], te[:, None, 1] - tr[None, :, 1])
        else:
            d = np.zeros((len(truths), len(tracks)))
        cost = np.minimum(d, c)
        rows, cols = linear_sum_assignment(cost) if d.size else ([], [])
        pair = {int(i): int(j) for i, j in zip(rows, cols) if d[i, j] < c}
        # Don't-care truths take their tracks out of the scoring.
        dropped = {j for i, j in pair.items() if not truths[i][5]}
        scored_truths = [i for i, x in enumerate(truths) if x[5]]
        live = [j for j in range(len(tracks)) if j not in dropped]
        assigned = {i: j for i, j in pair.items() if truths[i][5]}
        loc = sum(d[i, j] for i, j in assigned.items())
        missed = len(scored_truths) - len(assigned)
        spare = [j for j in live if j not in assigned.values()]
        if self.complete:
            false = len(spare)
        else:
            # Unlabelled: near no truth at all; they may be real traffic.
            false = 0
            self.unlabelled += len(spare)
        self.loc.append(loc)
        self.miss.append(c / 2 * missed)
        self.false.append(c / 2 * false)
        self.gospa.append(loc + c / 2 * (missed + false))

        self.truths += len(scored_truths)
        self.tracked += len(assigned)
        for i in scored_truths:
            if i in assigned:
                near = int(sum(1 for j in live if d[i, j] < c and np.argmin(d[:, j]) == i))
                self.near_counts.append(near)
        self.tracks_total += len(live)
        if self.complete:
            self.spurious += sum(1 for j in live if not (d[:, j] < c).any())
        for i, j in assigned.items():
            self.pos_err.append(d[i, j])
            _, _, _, tve, tvn, _ = truths[i]
            _, _, _, ve, vn = tracks[j]
            if ve is not None:
                self.vel_err.append(math.hypot(ve - tve, vn - tvn))
        for i in scored_truths:
            self.history[truths[i][0]].append((t, tracks[assigned[i]][0] if i in assigned else None))
            for j in live:
                if d[i, j] < c:
                    self.co[(truths[i][0], tracks[j][0])] += 1
        self.truth_samples += len(scored_truths)
        # A track counts against identity precision only where truth is complete.
        self.track_samples += len(live) if self.complete else sum(1 for j in live if (d[:, j] < c).any())

    def identity(self):
        """IDF1 (Ristani et al.): tracks matched one to one to truths over the
        whole run, maximising the samples each pair spends within the cutoff.
        ID recall is the share of truth samples on its matched track; ID
        precision the share of track samples on its matched truth."""
        if not self.co:
            return {"idf1": None, "id_precision": None, "id_recall": 0.0 if self.truth_samples else None}
        truths = sorted({k[0] for k in self.co})
        tracks = sorted({k[1] for k in self.co})
        ti, ki = {t: n for n, t in enumerate(truths)}, {u: n for n, u in enumerate(tracks)}
        m = np.zeros((len(truths), len(tracks)))
        for (t, u), n in self.co.items():
            m[ti[t], ki[u]] = n
        rows, cols = linear_sum_assignment(-m)
        idtp = float(m[rows, cols].sum())
        idr = idtp / self.truth_samples if self.truth_samples else None
        idp = idtp / self.track_samples if self.track_samples else None
        idf1 = 2 * idtp / (self.truth_samples + self.track_samples) if self.truth_samples + self.track_samples else None
        return {"idf1": idf1, "id_precision": idp, "id_recall": idr}

    def result(self, sample_secs):
        switches, fragments, longest, first = [], [], [], []
        hours = 0.0
        for k, h in self.history.items():
            uids = [u for _, u in h if u is not None]
            hours += len(h) * sample_secs / 3600
            if not uids:
                fragments.append(0)
                longest.append(0.0)
                continue
            switches.append(sum(1 for a, b in zip(uids, uids[1:]) if a != b))
            fragments.append(len(set(uids)))
            best = run = 1
            for a, b in zip(uids, uids[1:]):
                run = run + 1 if a == b else 1
                best = max(best, run)
            longest.append(best / len(h))
            first.append(next(t for t, u in h if u is not None) - h[0][0])
        mean = lambda xs: float(np.mean(xs)) if len(xs) else None  # noqa: E731
        rms = lambda xs: float(np.sqrt(np.mean(np.square(xs)))) if len(xs) else None  # noqa: E731
        out = {
            "samples": self.samples,
            "tracks": len(self.uids),
            "live_tracks_mean": mean(self.live),
            "live_truths_mean": (self.truths / self.samples) if self.samples else None,
            "gospa_cutoff_m": self.c,
            "gospa": mean(self.gospa),
            "gospa_localisation": mean(self.loc),
            "gospa_missed": mean(self.miss),
            "gospa_false": mean(self.false),
            "completeness": self.tracked / self.truths if self.truths else None,
            "ambiguity": mean(self.near_counts),
            "spuriousness": (self.spurious / self.tracks_total if self.tracks_total else 0.0) if self.complete else None,
            "unlabelled_track_samples": None if self.complete else self.unlabelled,
            "position_rms_m": rms(self.pos_err),
            "velocity_rms_mps": rms(self.vel_err),
            "truths": len(self.history),
            "id_changes_per_truth_hour": (sum(switches) / hours) if hours else None,
            "fragmentation": mean(fragments),
            "longest_segment": mean(longest),
            "time_to_first_track_s": float(np.median(first)) if first else None,
            **self.identity(),
        }
        return {k: (round(v, 4) if isinstance(v, float) else v) for k, v in out.items()}


def system_tracks(path, local):
    """{T: [(uid, e, n, ve, vn)]} of confirmed single-object system tracks."""
    by_t = defaultdict(list)
    contributors = defaultdict(list)
    for r in jsonl(path):
        if r.get("kind", "track") != "track":
            continue
        T = ts(r["t"])
        contributors[T].append((r["uid"], r.get("contributors") or []))
        if r.get("state") != "confirmed":
            continue
        e, n, ve, vn = dead_reckon(local, r["lat"], r["lon"], r.get("course"), r.get("speed"),
                                   T - ts(r["observed_at"]))
        by_t[T].append((r["uid"], e, n, ve, vn))
    return by_t, contributors


def source_tracks(path, local, times):
    """Per source: {T: [(key, e, n, ve, vn)]}, each source track's last report before T."""
    per = defaultdict(lambda: defaultdict(list))
    reports = defaultdict(list)
    for r in jsonl(path):
        reports[(r["source"], r["key"])].append((ts(r["t"]), r))
    for (src, key), rs in reports.items():
        rs.sort(key=lambda x: x[0])
        tlist = [x[0] for x in rs]
        for T in times:
            i = bisect.bisect_right(tlist, T) - 1
            if i < 0 or T - tlist[i] > SOURCE_STALE_S or rs[i][1].get("state") == "dropped":
                continue
            r = rs[i][1]
            e, n, ve, vn = dead_reckon(local, r["lat"], r["lon"], r.get("course"), r.get("speed"), T - tlist[i])
            per[src][T].append((key, e, n, ve, vn))
    return per, reports


def label_source_tracks(reports, truth, cutoff):
    """Which truth each source track follows: its key when a feed keys by the
    truth id, else the truth nearest most of its reports (four in five)."""
    labels = {}
    for (src, key), rs in reports.items():
        if key in truth.series:
            labels[f"{src}/{key}"] = key
            continue
        votes = Counter()
        for t, r in rs:
            e, n = truth.local.en(r["lat"], r["lon"])
            best = min(((math.hypot(e - x[1], n - x[2]), x[0]) for x in truth.at(t)), default=None)
            votes[best[1] if best and best[0] < cutoff else None] += 1
        total = sum(votes.values())
        label, count = votes.most_common(1)[0] if votes else (None, 0)
        if total >= 5 and count * 5 >= total * 4:
            labels[f"{src}/{key}"] = label if label is not None else "clutter"
    return labels


def labels_at(per_source, truth, cutoff):
    """Which truth each source track follows at each sample: its key when a
    feed keys by the truth id, else the nearest truth within the cutoff at
    that moment (smoothed over time, below). A tracker that swaps targets is
    judged on the one it follows at the time."""
    out = defaultdict(dict)
    for src, by_t in per_source.items():
        for T, rows in by_t.items():
            truths = truth.at(T)
            keyed = [(k, e, n) for k, e, n, *_ in rows if k in truth.series]
            for k, *_ in keyed:
                out[T][f"{src}/{k}"] = k
            rest = [(k, e, n) for k, e, n, *_ in rows if k not in truth.series]
            if not rest or not truths:
                continue
            d = np.array([[math.hypot(e - x[1], n - x[2]) for x in truths] for _, e, n in rest])
            # The nearest truth, not a one-to-one match: a sensor's two
            # fragments of one vessel both follow that vessel.
            for i, (k, _, _) in enumerate(rest):
                order = np.argsort(d[i])
                j = int(order[0])
                if d[i, j] >= cutoff:
                    out[T][f"{src}/{k}"] = "clutter"
                # Between two vessels (the next no more than twice as far):
                # no one can say which it follows, so it is not scored.
                elif len(order) < 2 or d[i, int(order[1])] > 2 * max(d[i, j], 1.0):
                    out[T][f"{src}/{k}"] = truths[j][0]
    # Smooth each source track's labels over time (the commonest within
    # LABEL_WINDOW samples): a noisy position near two vessels flips from
    # sample to sample, a tracker that swapped targets stays swapped.
    series = defaultdict(list)
    for T in sorted(out):
        for k, lab in out[T].items():
            series[k].append((T, lab))
    half = LABEL_WINDOW // 2
    for k, xs in series.items():
        labs = [lab for _, lab in xs]
        for i, (T, _) in enumerate(xs):
            out[T][k] = Counter(labs[max(0, i - half):i + half + 1]).most_common(1)[0][0]
    return out


# Samples each side of a source track's label that decide it.
LABEL_WINDOW = 21


def correlation(contributors, labels):
    right = wrong = missed = clutter = 0
    per_sample = isinstance(labels, dict) and labels and isinstance(next(iter(labels.values())), dict)
    whole_run = labels
    for T, tracks in contributors.items():
        if per_sample:
            labels = whole_run.get(T, {})
        where = defaultdict(set)  # truth -> system tracks holding its source tracks
        for uid, cs in tracks:
            ls = [(c, labels.get(c)) for c in cs if c.split("/")[-1] != "detections"]
            known = [(c, lab) for c, lab in ls if lab and lab != "clutter"]
            for a in range(len(known)):
                for b in range(a + 1, len(known)):
                    if known[a][0].split("/")[0] == known[b][0].split("/")[0]:
                        continue  # one sensor's two tracks: not a pairing question
                    if known[a][1] == known[b][1]:
                        right += 1
                    else:
                        wrong += 1
            if known:
                clutter += sum(1 for _, lab in ls if lab == "clutter")
            for _, lab in known:
                where[lab].add(uid)
        for lab, uids in where.items():
            if len(uids) > 1:
                missed += len(uids) - 1
    return {
        "pairing_precision": round(right / (right + wrong), 4) if right + wrong else None,
        "pairing_recall": round(right / (right + missed), 4) if right + missed else None,
        "pairs_right": right, "pairs_wrong": wrong, "pairings_missed": missed,
        "clutter_tracks_joined": clutter,
    }


def score(scenario_dir, run_dir):
    scenario = json.loads((scenario_dir / "scenario.json").read_text())
    run = json.loads((run_dir / "run.json").read_text())
    notes = scenario.get("notes") or {}
    cutoff = float(notes.get("gospa_cutoff_m", 100))
    complete = notes.get("truth_complete", True)
    sample = float(scenario.get("sample_secs", 1.0))
    truth = Truth(jsonl(scenario_dir / "truth.jsonl"), max_gap=max(30.0, 3 * sample))
    detected = None
    if notes.get("detected_within_m"):
        detected = truth.require_detection(jsonl(run_dir / "plots.jsonl"), float(notes["detected_within_m"]),
                                           float(notes.get("detected_window_s", 5.0)))

    tracks, contributors = system_tracks(run_dir / "tracks.jsonl", truth.local)
    times = sorted(contributors)
    system = Scorer(cutoff, complete)
    for T in times:
        system.add(T, truth.at(T), tracks.get(T, []))

    per_source, reports = source_tracks(run_dir / "source_tracks.jsonl", truth.local, times)
    trackers = {}
    for s in run["sources"]:
        if s.get("tracker") is None:
            continue
        sc = Scorer(cutoff, complete)
        for T in times:
            sc.add(T, truth.at(T), per_source[s["id"]].get(T, []))
        trackers[s["id"]] = sc.result(sample)

    out = {
        "scenario": scenario["name"],
        "description": scenario.get("description"),
        "truth_complete": complete,
        "truth_points_detected": detected,
        "speed_x_real_time": round(run["scenario_secs"] / run["wall_secs"], 1) if run["wall_secs"] else None,
        "wall_secs": round(run["wall_secs"], 2),
        "observations": run["observations"],
        "system": system.result(sample),
        "trackers": trackers,
    }
    if len(run["sources"]) > 1:
        # The gate's measure: each source track labelled by the truth it
        # follows four fifths of the time (others are not scored).
        out["correlation"] = correlation(contributors, label_source_tracks(reports, truth, cutoff))
        # Stricter: labelled at each moment (smoothed; skipped between two
        # vessels), so tracks that wander between vessels count too.
        out["correlation_per_sample"] = correlation(contributors, labels_at(per_source, truth, cutoff))
    (run_dir / "score.json").write_text(json.dumps(out, indent=2) + "\n")
    return out

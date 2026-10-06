#!/usr/bin/env python3
"""Turn Autoferry scenarios into OpenTrack replay fixtures.

The Autoferry sensor fusion benchmark (NTNU, CC0,
https://github.com/mhelg/sensor_fusion_dataset) has radar and lidar detections
from the milliAmpere ferry in Trondheim, with ground truth for two reference
vessels. Each scenario becomes two fixtures, both with a "track" feed made
from the ground truth (as an AIS-like feed would report it: every few
seconds, with noise, with a name):

- `scenarioN-tracks.jsonl`: lidar and radar detections run through a simple
  tracker first, so they arrive as anonymous source tracks (what a radar's
  own tracker outputs). Correlation has to pair them with the track feed.
- `scenarioN-detections.jsonl`: the raw detections, one observation each,
  clutter included. OpenTrack has to associate them with the tracks.

Each line is `{"feed": ..., "truth": <target id or null>, "obs": <observation>}`;
`truth` is the target the report is closest to (null for clutter), for scoring.

Usage: autoferry.py <dataset dir> <out dir> [scenario ...]   (default 2 16)
"""

import json
import math
import random
import sys
from pathlib import Path

# Origin of the Piren NED frame the dataset uses.
PIREN = (63.4389029083, 10.39908278)
A, F = 6378137.0, 1 / 298.257223563
E2 = F * (2 - F)

LIDAR, RADAR = 1, 2
NAMES = {1: {1: "HAVFRUEN", 2: "GUNNERUS"}, 2: {1: "HAVFRUEN", 2: "JETBOAT"}}
# Autoferry detections sit on the hull, not at the GPS antenna: several
# metres of error on a 30 m vessel. Reported uncertainty follows what the
# scenarios show (median/p90 against ground truth).
SENSORS = {
    LIDAR: {"name": "lidar", "cep_m": 8.0, "gate_m": 20.0, "cluster_m": 10.0, "drop_s": 5.0},
    RADAR: {"name": "radar", "cep_m": 10.0, "gate_m": 25.0, "cluster_m": 12.0, "drop_s": 8.0},
}
TRACK_PERIOD_S = 3.0
TRACK_NOISE_M = 2.0
TRACK_CEP_M = 10.0
LABEL_M = 30.0


def to_lla(n, e):
    lat0 = math.radians(PIREN[0])
    s = math.sin(lat0)
    rn = A / math.sqrt(1 - E2 * s * s)
    rm = rn * (1 - E2) / (1 - E2 * s * s)
    return PIREN[0] + math.degrees(n / rm), PIREN[1] + math.degrees(e / (rn * math.cos(lat0)))


def iso(t):
    from datetime import datetime, timezone

    return datetime.fromtimestamp(t, timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


def points(m):
    """A lidar/radar measurement as (north, east) pairs: the JSON drops the 2xM shape."""
    if not m:
        return []
    if isinstance(m[0], list):
        return list(zip(m[0], m[1]))
    return [(m[0], m[1])]


def observation(source, key, t, n, e, cep, **extra):
    lat, lon = to_lla(n, e)
    o = {
        "schema_version": 1,
        "source_id": source,
        "source_track_key": key,
        "observed_at": iso(t),
        "received_at": iso(t),
        "position": {"latitude": round(lat, 7), "longitude": round(lon, 7)},
        "uncertainty": {"circular_error_m": cep},
        "classification": {"domain": "surface"},
    }
    o.update(extra)
    return o


def kinematics(vn, ve):
    return {
        "course_deg": round(math.degrees(math.atan2(ve, vn)) % 360, 1),
        "speed_mps": round(math.hypot(vn, ve), 2),
    }


class Truth:
    """Ground truth per target, as a time series in the Piren frame."""

    def __init__(self, gt):
        self.series = {}
        for entry in gt:
            for t in entry if isinstance(entry, list) else [entry]:
                s = self.series.setdefault(t["targetID"], [])
                if not s or t["time"] > s[-1][0]:
                    s.append((t["time"], t["position"][0], t["position"][1]))

    def at(self, target, t):
        s = self.series[target]
        if t <= s[0][0] or t >= s[-1][0]:
            return None
        lo, hi = 0, len(s) - 1
        while hi - lo > 1:
            mid = (lo + hi) // 2
            if s[mid][0] <= t:
                lo = mid
            else:
                hi = mid
        (t0, n0, e0), (t1, n1, e1) = s[lo], s[hi]
        f = (t - t0) / (t1 - t0) if t1 > t0 else 0
        return n0 + f * (n1 - n0), e0 + f * (e1 - e0)

    def label(self, t, n, e):
        best = None
        for k in self.series:
            p = self.at(k, t)
            if p is None:
                continue
            d = math.hypot(n - p[0], e - p[1])
            if d <= LABEL_M and (best is None or d < best[0]):
                best = (d, k)
        return best and best[1]


def track_feed(truth, env, rng):
    """The ground truth as a track feed: every few seconds, with noise and a name."""
    out = []
    for k, s in truth.series.items():
        t = s[0][0] + 1.0
        while t < s[-1][0] - 1.0:
            p, before, after = truth.at(k, t), truth.at(k, t - 1.5), truth.at(k, t + 1.5)
            if p and before and after:
                n, e = p[0] + rng.gauss(0, TRACK_NOISE_M), p[1] + rng.gauss(0, TRACK_NOISE_M)
                o = observation(
                    "track",
                    f"target-{k}",
                    t,
                    n,
                    e,
                    TRACK_CEP_M,
                    name=NAMES[env][k],
                    kinematics=kinematics((after[0] - before[0]) / 3, (after[1] - before[1]) / 3),
                )
                out.append({"feed": "track", "truth": k, "obs": o})
            t += TRACK_PERIOD_S
    return out


def scans(detections, sensor):
    for d in detections:
        if d["sensorID"] == sensor:
            own = d["ownshipPosition"]
            yield d["time"], [(n + own[0], e + own[1]) for n, e in points(d["measurement"])]


def detection_feed(detections, truth, sensor):
    cfg = SENSORS[sensor]
    out = []
    for i, (t, pts) in enumerate(scans(detections, sensor)):
        for j, (n, e) in enumerate(pts):
            o = observation(f"{cfg['name']}-det", f"s{i}-{j}", t, n, e, cfg["cep_m"])
            out.append({"feed": o["source_id"], "truth": truth.label(t, n, e), "obs": o})
    return out


def cluster(pts, radius):
    """Merge detections of one scan that are closer than `radius` (one extended target)."""
    groups = []
    for p in pts:
        for g in groups:
            c = (sum(x for x, _ in g) / len(g), sum(y for _, y in g) / len(g))
            if math.hypot(p[0] - c[0], p[1] - c[1]) < radius:
                g.append(p)
                break
        else:
            groups.append([p])
    return [(sum(x for x, _ in g) / len(g), sum(y for _, y in g) / len(g)) for g in groups]


class Track:
    def __init__(self, tid, t, n, e):
        self.id, self.t, self.n, self.e, self.vn, self.ve = tid, t, n, e, 0.0, 0.0
        self.hits = [t]
        self.confirmed = False

    def predict(self, t):
        dt = t - self.t
        return self.n + self.vn * dt, self.e + self.ve * dt


def tracker_feed(detections, truth, sensor, alpha=0.5, beta=0.2):
    """A nearest-neighbour alpha-beta tracker, like the one inside a radar: confirmed
    after 3 hits in 5 scans, dropped after a few seconds without one."""
    cfg = SENSORS[sensor]
    out, tracks, next_id = [], [], 1
    for t, pts in scans(detections, sensor):
        pts = cluster(pts, cfg["cluster_m"])
        pairs = sorted(
            (math.hypot(p[0] - pn, p[1] - pe), ti, pi)
            for ti, tr in enumerate(tracks)
            for pn, pe in [tr.predict(t)]
            for pi, p in enumerate(pts)
        )
        used_t, used_p = set(), set()
        for d, ti, pi in pairs:
            if d > cfg["gate_m"] or ti in used_t or pi in used_p:
                continue
            used_t.add(ti)
            used_p.add(pi)
            tr, (n, e) = tracks[ti], pts[pi]
            dt = max(t - tr.t, 1e-3)
            pn, pe = tr.predict(t)
            rn, re = n - pn, e - pe
            if len(tr.hits) == 1:
                tr.vn, tr.ve = (n - tr.n) / dt, (e - tr.e) / dt
                tr.n, tr.e = n, e
            else:
                tr.n, tr.e = pn + alpha * rn, pe + alpha * re
                tr.vn, tr.ve = tr.vn + beta * rn / dt, tr.ve + beta * re / dt
            tr.t = t
            tr.hits.append(t)
            recent = [h for h in tr.hits if t - h <= 5 * cfg["drop_s"] / 2]
            tr.confirmed = tr.confirmed or len(recent) >= 3
            if tr.confirmed:
                o = observation(
                    cfg["name"],
                    f"{cfg['name'][0].upper()}{tr.id}",
                    t,
                    tr.n,
                    tr.e,
                    cfg["cep_m"],
                    kinematics=kinematics(tr.vn, tr.ve),
                )
                out.append({"feed": cfg["name"], "truth": truth.label(t, tr.n, tr.e), "obs": o})
        tracks = [
            tr for tr in tracks if t - tr.hits[-1] <= (cfg["drop_s"] if tr.confirmed else 2 * cfg["drop_s"] / 5)
        ]
        for pi, p in enumerate(pts):
            if pi not in used_p:
                tracks.append(Track(next_id, t, *p))
                next_id += 1
    return out


def write(path, rows):
    rows.sort(key=lambda r: r["obs"]["observed_at"])
    with open(path, "w") as f:
        for r in rows:
            f.write(json.dumps(r, separators=(",", ":")) + "\n")
    print(f"{path}: {len(rows)} reports")


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    data, out = Path(sys.argv[1]), Path(sys.argv[2])
    out.mkdir(parents=True, exist_ok=True)
    for sc in [int(s) for s in sys.argv[3:]] or [2, 16]:
        base = data / f"scenario{sc}" / f"scenario{sc}"
        detections = json.load(open(f"{base}_detections.json"))
        truth = Truth(json.load(open(f"{base}_groundTruth.json")))
        env = 1 if sc <= 6 else 2
        feed = track_feed(truth, env, random.Random(sc))
        write(
            out / f"scenario{sc}-tracks.jsonl",
            feed + tracker_feed(detections, truth, LIDAR) + tracker_feed(detections, truth, RADAR),
        )
        write(
            out / f"scenario{sc}-detections.jsonl",
            feed + detection_feed(detections, truth, LIDAR) + detection_feed(detections, truth, RADAR),
        )


if __name__ == "__main__":
    main()

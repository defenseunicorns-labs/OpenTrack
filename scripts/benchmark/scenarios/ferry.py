"""Autoferry: recorded lidar and radar from the milliAmpere ferry in
Trondheim, with ground truth for the reference vessels (NTNU, CC0:
https://github.com/Autoferry/sensor_fusion_dataset).

- `autoferry-N`: the lidar and radar plots only, each through its own
  tracker stage (as docs/examples/autoferry). Scores the trackers and the
  engine's fusion of their tracks.
- `autoferry-N-fusion`: the same, plus a vessel track feed made from the
  ground truth (every 3 s, 2 m noise, named, as an AIS feed would report it).
  Scores correlation: each vessel's lidar and radar tracks should join its
  feed track.
"""

import json
import random
import sys

from common import REPO, Writer, fetch, iso, raw_dir, spec, ts

sys.path.insert(0, str(REPO / "scripts/benchmark/replay"))
import autoferry as af  # noqa: E402

URL = "https://raw.githubusercontent.com/Autoferry/sensor_fusion_dataset/main/scenario{n}/scenario{n}_{kind}.json"
NUMBERS = [2, 3, 4, 5, 6, 13, 16, 17, 22]
TRUTH_STEP_S = 1.0


def load(n):
    d = raw_dir() / "autoferry"
    det = json.loads(fetch(URL.format(n=n, kind="detections"), d / f"scenario{n}_detections.json").read_text())
    gt = json.loads(fetch(URL.format(n=n, kind="groundTruth"), d / f"scenario{n}_groundTruth.json").read_text())
    return det, af.Truth(gt)


def sensor_spec(name):
    example = json.loads((REPO / f"docs/examples/autoferry/demo-autoferry-{name}.json").read_text())
    s = spec("radar-plots", id=name, name=f"Autoferry {name}")
    s["pipeline"]["tracker"] = example["pipeline"]["tracker"]
    return s


def build(n, fusion):
    name = f"autoferry-{n}" + ("-fusion" if fusion else "")
    print(name)
    det, truth = load(n)
    w = Writer(name)
    env = 1 if n <= 6 else 2
    for sensor, label in [(af.LIDAR, "lidar"), (af.RADAR, "radar")]:
        cep = af.SENSORS[sensor]["cep_m"]
        for i, (t, pts) in enumerate(af.scans(det, sensor)):
            plots = []
            for j, (pn, pe) in enumerate(pts):
                lat, lon = af.to_lla(pn, pe)
                plots.append({"id": f"s{i}-{j}", "t": iso(t), "lat": round(lat, 7), "lon": round(lon, 7), "cep": cep})
            w.frame(label, t, json_={"plots": plots})
    for k, s in truth.series.items():
        t = s[0][0]
        while t <= s[-1][0]:
            p = truth.at(k, t)
            if p:
                lat, lon = af.to_lla(*p)
                w.truth_point(t, f"target-{k}", lat, lon)
            t += TRUTH_STEP_S
    sources = [sensor_spec("lidar"), sensor_spec("radar")]
    if fusion:
        rng = random.Random(n)
        for row in af.track_feed(truth, env, rng):
            o = row["obs"]
            k = o.get("kinematics", {})
            t = ts(o["observed_at"])
            w.frame("vessels", t, json_={
                "id": o["source_track_key"], "name": (af.NAMES.get(env) or {}).get(row["truth"]),
                "t": o["observed_at"], "lat": o["position"]["latitude"], "lon": o["position"]["longitude"],
                "course": k.get("course_deg"), "speed": k.get("speed_mps"), "cep": af.TRACK_CEP_M,
                "domain": "surface",
            })
        sources.insert(0, spec("track-feed", id="vessels", name="Autoferry vessel feed"))
    w.finish(
        f"Autoferry scenario {n}: recorded lidar and radar"
        + (" with a vessel track feed from the ground truth" if fusion else "") + ".",
        sources,
        engine={"drop_after_secs": 60, "stale_surface_secs": 15},
        notes={"dataset": "https://github.com/Autoferry/sensor_fusion_dataset (CC0)", "truth_complete": True,
               "gospa_cutoff_m": 50},
    )


SCENARIOS = {}
for _n in NUMBERS:
    SCENARIOS[f"autoferry-{_n}"] = (lambda n=_n: build(n, False))
    SCENARIOS[f"autoferry-{_n}-fusion"] = (lambda n=_n: build(n, True))


"""Synthetic scenarios: generated truth, simulated radars, no data to fetch.

- `synthetic-crossing`: twelve surface targets that all pass close to one
  point mid-run (a quarter of them turn after it), seen by two radars with
  heavy clutter. The hard case for association and track identity.
"""

import math
import random
from datetime import datetime, timezone

from common import Local, Writer
from radar import Radar
from scenarios.stonesoup import interpolate, radar_frames, radar_spec, truth_points


def crossing():
    name = "synthetic-crossing"
    print(name)
    rng = random.Random(7)
    centre = Local(36.85, -76.0)  # off Hampton Roads
    reports = {}
    t0 = datetime(2026, 1, 1, tzinfo=timezone.utc).timestamp()
    for k in range(12):
        # Every target passes within a few hundred metres of the centre mid-run.
        heading = math.radians(k * 30 + rng.uniform(-8, 8))
        speed = rng.uniform(5, 15)
        miss = rng.uniform(-300, 300)
        pts = []
        for s in range(0, 1201, 5):
            d = (s - 600) * speed
            e = d * math.sin(heading) + miss * math.cos(heading)
            n = d * math.cos(heading) - miss * math.sin(heading)
            if k % 4 == 0 and s > 600:  # a quarter turn after the crossing
                turn = min((s - 600) / 120, 1) * math.pi / 4
                e2 = e * math.cos(turn) - n * math.sin(turn)
                n = e * math.sin(turn) + n * math.cos(turn)
                e = e2
            lat, lon = centre.ll(e, n)
            pts.append((t0 + s, lat, lon, None))
        reports[f"T{k + 1:02d}"] = pts
    truth_at = interpolate(reports, 1.0, 10.0)
    radars = [Radar("north", *centre.ll(0, 15_000), revisit_s=2.5, range_max_m=40_000, sigma_range_m=15,
                    sigma_bearing_deg=0.4, pd=0.8, clutter_per_scan=40.0, seed=31),
              Radar("west", *centre.ll(-18_000, 0), revisit_s=3.0, range_max_m=40_000, sigma_range_m=20,
                    sigma_bearing_deg=0.3, pd=0.8, clutter_per_scan=40.0, seed=32)]
    w = Writer(name)
    radar_frames(w, radars, truth_at, 1.0)
    truth_points(w, radars, truth_at, 1)
    tracker = {"algorithm": "mht", "measurement_sigma_m": 60.0, "clutter_density": 1e-7}
    w.finish(
        "Synthetic: twelve surface targets crossing near one point (some turning) seen by two radars "
        "with heavy clutter (40 false plots a scan) and 80% detection.",
        [radar_spec(r, "surface", tracker) for r in radars],
        engine={"drop_after_secs": 120, "stale_surface_secs": 20},
        notes={"truth_complete": True, "gospa_cutoff_m": 300, "radars": [r.settings() for r in radars]},
    )


SCENARIOS = {"synthetic-crossing": crossing}

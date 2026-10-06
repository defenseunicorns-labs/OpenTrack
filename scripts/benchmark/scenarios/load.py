"""Load scenarios: N targets reporting once a second (an AIS-like feed with an
MMSI each), spread over the North Atlantic so they are not all neighbours.
One frame per second carries every report, as a batched feed would. These
measure throughput, not quality: the run records observations per second
and is not scored.

- `load-2k`, `load-8k`, `load-16k`: 2,000 / 8,000 / 16,000 tracks, 60 s.
"""

import math
import random
from datetime import datetime, timezone

from common import Writer, iso, spec

SECONDS = 60


def build(n):
    name = f"load-{n // 1000}k"
    print(name)
    rng = random.Random(n)
    t0 = datetime(2026, 1, 1, tzinfo=timezone.utc).timestamp()
    targets = []
    for k in range(n):
        lat, lon = rng.uniform(30, 55), rng.uniform(-60, -10)
        course, speed = rng.uniform(0, 360), rng.uniform(2, 15)
        targets.append((f"{200000000 + k}", lat, lon, course, speed))
    w = Writer(name)
    for s in range(SECONDS):
        t = t0 + s
        recs = []
        for mmsi, lat, lon, course, speed in targets:
            d = speed * s
            r = math.radians(course)
            la = lat + math.degrees(d * math.cos(r) / 6378137.0)
            lo = lon + math.degrees(d * math.sin(r) / (6378137.0 * math.cos(math.radians(lat))))
            recs.append({"id": mmsi, "t": iso(t), "lat": round(la, 6), "lon": round(lo, 6),
                         "course": round(course, 1), "speed": round(speed, 2), "cep": 10.0, "domain": "surface"})
        w.frame("feed", t, json_={"reports": recs})
    feed = spec("track-feed", id="feed", name=f"Load: {n} tracks at 1 Hz")
    feed["pipeline"]["codec"] = {"type": "json", "records": "reports"}
    feed["pipeline"]["mapping"]["rules"][0]["identifiers"] = [{"scheme": "mmsi", "value": "id"}]
    w.finish(f"Load: {n} surface tracks reporting once a second for {SECONDS} s.", [feed],
             sample_secs=30.0, notes={"load": True})


SCENARIOS = {f"load-{n // 1000}k": (lambda n=n: build(n)) for n in (2000, 8000, 16000)}

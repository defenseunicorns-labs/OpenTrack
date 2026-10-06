"""Synthetic acoustic-array scenarios: small UAS ("drones") heard by fixed
acoustic arrays reporting bearing (+ often range), fused by OpenTrack's
non-point-contact correlation (`docs/non-point-contacts.md`, "Acoustic
arrays"; see also `scripts/demo-sapient-acoustic.sh` for the real SAPIENT
feed this stands in for). Parameters are cited public figures, see
`acoustic.py`'s docstring.

- `acoustic-sparse`: 3 arrays (the minimum a bearing-only fix needs), 2
  drones. The easy case: is basic bearing/range fusion right at all.
- `acoustic-demo`: 5 arrays, 6 drones: two orbits that cross, a formation
  pair 70-80 m apart, a loiterer and one drone transiting the field.
- `acoustic-dense`: 8 arrays over a wider area, 14 drones: four orbits, two
  formation pairs, two loiterers and four drones transiting straight
  through, several leaving every array's range partway through. The hard
  case: many nearby objects, and tracks that must start and end cleanly.

All durations 3.5-5 minutes, 1 Hz reports, seeded for repeatable runs.
"""

import math

from common import Local, Writer
from acoustic import AcousticArray, acoustic_frames, acoustic_spec


def orbit_path(local, cn, ce, r, speed, direction, phase, alt):
    def f(t):
        ang = phase + direction * speed * t / r
        n, e = cn + r * math.cos(ang), ce + r * math.sin(ang)
        return (*local.ll(e, n), alt)

    return f


def loiter_path(local, cn, ce, rn, re, period_s, alt, phase=0.0):
    def f(t):
        n = cn + rn * math.sin(2 * math.pi * t / period_s + phase)
        e = ce + re * math.cos(2 * math.pi * t / (period_s * 1.3) + phase)
        return (*local.ll(e, n), alt)

    return f


def transit_path(local, n0, e0, heading_deg, speed, alt):
    hd = math.radians(heading_deg)
    ch, sh = math.cos(hd), math.sin(hd)

    def f(t):
        n, e = n0 + speed * t * ch, e0 + speed * t * sh
        return (*local.ll(e, n), alt)

    return f


def build(name, description, lat0, lon0, array_defs, drones, duration, cutoff_m):
    print(name)
    local = Local(lat0, lon0)
    arrays = {}
    specs = []
    port = 47200
    for i, (aid, n, e, alt, range_m, sigma_bearing, ranged_share, false_rate) in enumerate(array_defs):
        lat, lon = local.ll(e, n)
        arr = AcousticArray(aid, lat, lon, alt, range_m=range_m, sigma_bearing_deg=sigma_bearing,
                             ranged_share=ranged_share, false_rate_hz=false_rate, seed=1000 + i)
        arrays[aid] = arr
        specs.append(acoustic_spec(arr, aid, port + i))
    paths = {did: p for did, p in drones}
    w = Writer(name)
    truth = acoustic_frames(w, arrays, paths, 0.0, duration, step=1.0)
    for did, points in truth.items():
        for t, lat, lon, alt, scored in points:
            w.truth_point(t, did, lat, lon, alt, score=scored)
    w.finish(
        description,
        specs,
        notes={
            "truth_complete": True,
            "gospa_cutoff_m": cutoff_m,
            "arrays": [{"id": aid, **arr.settings()} for aid, arr in arrays.items()],
            "drones": len(paths),
        },
    )


def sparse():
    lat0, lon0 = 34.05, -117.75
    local = Local(lat0, lon0)
    arrays = [
        ("a1", 80.0, 0.0, 2.0, 350.0, 2.5, 0.6, 0.01),
        ("a2", -40.0, 69.3, 2.0, 350.0, 3.0, 0.6, 0.01),
        ("a3", -40.0, -69.3, 2.0, 350.0, 3.5, 0.6, 0.01),
    ]
    drones = [
        ("d0", orbit_path(local, 0.0, 0.0, 180.0, 10.0, 1.0, 0.0, 60.0)),
        ("d1", loiter_path(local, 50.0, -60.0, 60.0, 40.0, 90.0, 40.0, phase=1.0)),
    ]
    build("acoustic-sparse", "Synthetic: 3 acoustic arrays (the minimum for a bearing-only fix), "
          "2 small UAS orbiting/loitering within 350 m. Small-UAS acoustic range ~300-500 m, bearing "
          "std 2-5 deg, 1 Hz reports (see acoustic.py).", lat0, lon0, arrays, drones, 210.0, 100.0)


def demo():
    lat0, lon0 = 34.08, -117.80
    local = Local(lat0, lon0)
    arrays = [
        ("a1", 0.0, 0.0, 2.0, 400.0, 3.0, 0.6, 0.01),
        ("a2", 150.0, 0.0, 2.0, 400.0, 2.5, 0.6, 0.01),
        ("a3", -150.0, 0.0, 2.0, 400.0, 3.5, 0.6, 0.012),
        ("a4", 0.0, 150.0, 2.0, 400.0, 3.0, 0.6, 0.01),
        ("a5", 0.0, -150.0, 2.0, 400.0, 4.0, 0.6, 0.01),
    ]
    drones = [
        ("d0", orbit_path(local, 0.0, 0.0, 350.0, 12.0, 1.0, 0.0, 80.0)),
        ("d1", orbit_path(local, 100.0, -80.0, 200.0, 8.0, -1.0, 1.0, 50.0)),
        ("d2", orbit_path(local, -150.0, 150.0, 150.0, 9.0, 1.0, 0.0, 55.0)),
        ("d3", orbit_path(local, -150.0, 150.0, 150.0, 9.0, 1.0, 0.53, 60.0)),
        ("d4", loiter_path(local, 200.0, 200.0, 50.0, 50.0, 70.0, 35.0)),
        ("d5", transit_path(local, -500.0, -500.0, 45.0, 15.0, 70.0)),
    ]
    build("acoustic-demo", "Synthetic: 5 acoustic arrays, 6 small UAS: two crossing orbits, a formation "
          "pair ~80 m apart, a loiterer and one drone transiting the field. Small-UAS acoustic range "
          "~300-500 m, bearing std 2-5 deg, speed 5-20 m/s, 1 Hz reports (see acoustic.py).",
          lat0, lon0, arrays, drones, 240.0, 100.0)


def dense():
    lat0, lon0 = 34.12, -117.85
    local = Local(lat0, lon0)
    arrays = [
        ("a1", -200.0, -450.0, 2.0, 450.0, 3.0, 0.55, 0.012),
        ("a2", -200.0, -150.0, 2.0, 450.0, 3.5, 0.55, 0.012),
        ("a3", -200.0, 150.0, 2.0, 450.0, 2.5, 0.55, 0.01),
        ("a4", -200.0, 450.0, 2.0, 450.0, 4.0, 0.55, 0.014),
        ("a5", 200.0, -450.0, 2.0, 450.0, 3.0, 0.55, 0.01),
        ("a6", 200.0, -150.0, 2.0, 450.0, 3.5, 0.55, 0.012),
        ("a7", 200.0, 150.0, 2.0, 450.0, 5.0, 0.55, 0.015),
        ("a8", 200.0, 450.0, 2.0, 450.0, 3.0, 0.55, 0.01),
    ]
    drones = [
        ("d0", orbit_path(local, -150.0, -300.0, 120.0, 10.0, 1.0, 0.0, 50.0)),
        ("d1", orbit_path(local, 150.0, -300.0, 100.0, 14.0, -1.0, 0.7, 60.0)),
        ("d2", orbit_path(local, -150.0, 300.0, 130.0, 9.0, 1.0, 1.4, 70.0)),
        ("d3", orbit_path(local, 150.0, 300.0, 110.0, 16.0, -1.0, 2.1, 45.0)),
        ("d4", orbit_path(local, 0.0, -100.0, 90.0, 11.0, 1.0, 0.0, 55.0)),
        ("d5", orbit_path(local, 0.0, -100.0, 90.0, 11.0, 1.0, 0.78, 58.0)),
        ("d6", orbit_path(local, 0.0, 100.0, 90.0, 13.0, -1.0, 0.0, 65.0)),
        ("d7", orbit_path(local, 0.0, 100.0, 90.0, 13.0, -1.0, 0.68, 68.0)),
        ("d8", loiter_path(local, -300.0, 0.0, 60.0, 40.0, 80.0, 40.0)),
        ("d9", loiter_path(local, 300.0, 0.0, 50.0, 70.0, 100.0, 48.0, phase=2.0)),
        ("d10", transit_path(local, -900.0, -600.0, 80.0, 18.0, 75.0)),
        ("d11", transit_path(local, 700.0, 0.0, 185.0, 12.0, 55.0)),
        ("d12", transit_path(local, -700.0, 700.0, 320.0, 20.0, 90.0)),
        ("d13", transit_path(local, 0.0, -900.0, 5.0, 8.0, 35.0)),
    ]
    build("acoustic-dense", "Synthetic: 8 acoustic arrays over a wider field, 14 small UAS: four orbits, "
          "two formation pairs 60-70 m apart, two loiterers and four drones transiting straight through "
          "(several leaving every array's range partway through). Small-UAS acoustic range ~300-500 m, "
          "bearing std 2-5 deg, speed 5-20 m/s, 1 Hz reports (see acoustic.py).",
          lat0, lon0, arrays, drones, 300.0, 100.0)


SCENARIOS = {"acoustic-sparse": sparse, "acoustic-demo": demo, "acoustic-dense": dense}

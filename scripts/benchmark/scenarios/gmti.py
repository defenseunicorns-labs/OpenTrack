"""GMTI and GPS of one exercise (Adelaide, October 2015): STANAG 4607 dwells
recorded from the air over Garden Island, and GPS logs of vehicles and a
kayak taking part. The GPS is the truth. The recordings are local (not
downloadable): set OT_BENCH_GMTI and OT_BENCH_GPS, or keep them in
~/data/gmti/STANAG4607 and ~/data/gps-all.

- `gmti-<area>`: the GMTI alone through the tracker stage. The radar also
  sees traffic that carried no GPS, so tracks away from every GPS vehicle
  are reported as unlabelled, not false.
- `gmti-<area>-fusion`: plus the GPS as a track feed (one fix per message),
  as scripts/benchmark/replay/gmti-gps-replay.py replays it. Scores GMTI-to-GPS correlation.

GPS points only count while the vehicle moves (MDV_MPS) inside the footprint
of a dwell within DWELL_WINDOW_S: otherwise the radar could not have seen it.
"""

import importlib.util
import json
import math
import os
import struct
from pathlib import Path

from common import REPO, Local, Writer, iso, spec

_spec = importlib.util.spec_from_file_location("gmti_gps_replay", REPO / "scripts/benchmark/replay/gmti-gps-replay.py")
replay = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(replay)

GMTI = Path(os.environ.get("OT_BENCH_GMTI", Path.home() / "data/gmti/STANAG4607")).expanduser()
GPS = Path(os.environ.get("OT_BENCH_GPS", Path.home() / "data/gps-all")).expanduser()
DWELL_WINDOW_S = 15.0
# Below this a ground mover is lost in the clutter notch: an MTI radar does
# not report it, so a parked vehicle is not a missed one.
MDV_MPS = 1.5

# Dwell segment fields D2..D27 in order, with their sizes in bytes.
DWELL_SIZES = [2, 2, 1, 2, 4, 4, 4, 4, 4, 4, 4, 4, 2, 2, 4, 1, 1, 2, 2, 2, 2, 2, 4, 4, 2, 2]

AREAS = {
    "garden-island-1013": {
        "gmti": ("2015-10-13", "*GardenIsland_3[47]*"),
        "gps": ["2015-10-13/ABI and NGA scenarios/13-Oct-15/*.gpx", "2015-10-13/SNA activities/*_1*.gpx"],
        "label": "Garden Island, 13 Oct 2015",
    },
    "garden-island-1014": {
        "gmti": ("2015-10-14", "*GardenIsland_*"),
        "gps": ["2015-10-14/**/*.gpx"],
        "label": "Garden Island, 14 Oct 2015",
    },
}


def dwell_area(pkt):
    """(sensor lat, lon, centre lat, lon, range half extent m, angle half extent deg) per dwell."""
    out = []
    j = replay.HEADER
    while j + replay.SEGMENT_HEADER <= len(pkt):
        kind, seg = pkt[j], struct.unpack(">I", pkt[j + 1 : j + 5])[0]
        body = pkt[j + replay.SEGMENT_HEADER : j + seg]
        if kind == replay.DWELL and len(body) > 8:
            mask = int.from_bytes(body[:8], "big")
            has = lambda n: bool(mask >> (63 - (n - 2)) & 1)  # noqa: E731
            off, at = 8, {}
            for n, size in enumerate(DWELL_SIZES, start=2):
                if has(n):
                    at[n] = off
                    off += size
            if all(k in at for k in (7, 8, 24, 25, 26, 27)) and off <= len(body):
                i32 = lambda n: struct.unpack(">i", body[at[n] : at[n] + 4])[0]  # noqa: E731
                u32 = lambda n: struct.unpack(">I", body[at[n] : at[n] + 4])[0]  # noqa: E731
                u16 = lambda n: struct.unpack(">H", body[at[n] : at[n] + 2])[0]  # noqa: E731
                lon = lambda raw: (raw * 360.0 / 2**32 + 180) % 360 - 180  # noqa: E731
                r = u16(26)
                rh = (r & 0x7FFF) / 128.0 * 1000.0 * (-1 if r & 0x8000 else 1)
                out.append((
                    i32(7) * 180.0 / 2**32, lon(u32(8)),
                    i32(24) * 180.0 / 2**32, lon(u32(25)),
                    rh, u16(27) * 360.0 / 65536.0,
                ))
        j += max(seg, replay.SEGMENT_HEADER)
    return out


class Footprints:
    def __init__(self):
        self.dwells = []  # (t, local frame at sensor, centre range, centre bearing, rh, ah)

    def add(self, t, area):
        slat, slon, clat, clon, rh, ah = area
        f = Local(slat, slon)
        e, n = f.en(clat, clon)
        self.dwells.append((t, f, math.hypot(e, n), math.degrees(math.atan2(e, n)), rh, ah))

    def sees(self, t, lat, lon):
        for td, f, rc, bc, rh, ah in self.dwells:
            if abs(td - t) > DWELL_WINDOW_S:
                continue
            e, n = f.en(lat, lon)
            r, b = math.hypot(e, n), math.degrees(math.atan2(e, n))
            if abs(r - rc) <= rh and abs((b - bc + 180) % 360 - 180) <= ah:
                return True
        return False


def build(area, fusion):
    a = AREAS[area]
    name = f"gmti-{area}" + ("-fusion" if fusion else "")
    print(name)
    day, pattern = a["gmti"]
    files = sorted((GMTI / day).glob(pattern))
    if not files:
        raise SystemExit(f"{name}: no GMTI recordings {GMTI / day / pattern} (set OT_BENCH_GMTI)")
    w = Writer(name)
    fp = Footprints()
    times = []
    pending = []
    for t, pkt in replay.gmti_packets(files):
        pending.append(pkt)
        if t is None:
            continue
        times.append(t)
        for p in pending:
            w.frame("gmti", t, hex_=p.hex())
            for d in dwell_area(p):
                fp.add(t, d)
        pending = []
    start, end = min(times), max(times)
    gpx = sorted({p for g in a["gps"] for p in GPS.glob(g)})
    labelled, seen = 0, 0
    for path in gpx:
        pts = [p for p in replay.gpx_points(path) if start <= p[0] <= end]
        if len(pts) < 2:
            continue
        dev = replay.device(path)
        domain = "surface" if dev == "KAYAK" else "ground"
        labelled += 1
        for i, (t, lat, lon) in enumerate(pts):
            course, speed = replay.motion(pts, i)
            visible = speed is not None and speed >= MDV_MPS and fp.sees(t, lat, lon)
            seen += visible
            w.truth_point(t, dev, lat, lon, score=visible)
            if fusion:
                msg = {"id": dev, "name": dev, "t": iso(t), "lat": lat, "lon": lon, "course": course,
                       "speed": speed, "domain": domain, "cep": 5.0}
                if i == len(pts) - 1:
                    msg["state"] = "dropped"
                w.frame("gps", t, json_=msg)
    if not labelled:
        raise SystemExit(f"{name}: no GPS logs overlap the GMTI ({GPS}; set OT_BENCH_GPS)")
    print(f"  {labelled} GPS devices, {seen} points moving inside a dwell footprint")
    sources = [spec("stanag4607")]
    if fusion:
        g = json.loads((REPO / "docs/examples/gps-udp.json").read_text())
        g["id"] = "gps"
        sources.insert(0, g)
    w.finish(
        f"{a['label']}: STANAG 4607 GMTI" + (" with the exercise GPS as a track feed" if fusion else "")
        + "; GPS logs as truth.",
        sources,
        engine={"drop_after_secs": 120},
        step_secs=1.0,
        sample_secs=2.0,
        notes={"truth_complete": False, "gospa_cutoff_m": 200,
               "detected_within_m": 150, "detected_window_s": 5,
               "recordings": [f.name for f in files], "gps": [p.name for p in gpx]},
    )


SCENARIOS = {}
for _a in AREAS:
    SCENARIOS[f"gmti-{_a}"] = (lambda a=_a: build(a, False))
    SCENARIOS[f"gmti-{_a}-fusion"] = (lambda a=_a: build(a, True))

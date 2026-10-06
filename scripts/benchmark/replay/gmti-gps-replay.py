#!/usr/bin/env python3
"""Replay GMTI recordings and GPS logs of the same exercise together, on one
clock, as if happening now: a test of GPS/GMTI correlation on real data.

- GMTI: a TCP server (like gmti-rebroadcast.py) streams the STANAG 4607
  packets of the recordings, in order, paced by their dwell times, to an
  OpenTrack source with the stanag4607 codec and `shift_to_now` (see
  docs/examples/stanag4607.json). The codec shifts every record so the first
  dwell is "now"; the replay starts when the source connects, so that is the
  replay's start.
- GPS: every track point inside the recordings' time span goes as JSON over UDP
  to a track feed source (docs/examples/gps-udp.json) at the same moment,
  with its time shifted the same way: {"id", "name", "t", "lat", "lon",
  "course", "speed", "domain"}. Each device's track ends with `state: dropped`.

Usage: gmti-gps-replay.py --gmti <file.4607>... --gpx <file.gpx>...
           [--port 4607] [--gps 127.0.0.1:47010] [--speed 1.0] [--loops 0]

The recordings should be contiguous (no gaps are cut, so both streams keep
the same clock). Each loop starts again when it ends; the GMTI codec re-anchors
on the jump back in time and the GPS keys get the loop number (`TM01-r2`).
"""

import argparse
import json
import math
import re
import socket
import struct
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path

HEADER, SEGMENT_HEADER = 32, 5
MISSION, DWELL = 1, 2

PT = re.compile(r"<trkpt ([^>]*)>(.*?)</trkpt>", re.S)
ATTR = re.compile(r'(lat|lon)="([-\d.]+)"')
TIME = re.compile(r"<time>([^<]+)</time>")


def gmti_packets(paths):
    """(recording time in s since the epoch or None, packet) in file order.
    Dwell times count from the latest mission segment's reference date."""
    day = None
    for path in paths:
        data = path.read_bytes()
        i = 0
        while i + 6 <= len(data):
            size = struct.unpack(">I", data[i + 2 : i + 6])[0]
            if size < HEADER or i + size > len(data):
                raise SystemExit(f"{path.name}: bad packet size {size} at byte {i}")
            pkt, t = data[i : i + size], None
            j = HEADER
            while j + SEGMENT_HEADER <= size:
                kind, seg = pkt[j], struct.unpack(">I", pkt[j + 1 : j + 5])[0]
                body = pkt[j + SEGMENT_HEADER : j + seg]
                if kind == MISSION and len(body) >= 39:
                    # Mission plan (12), flight plan (12), platform type (1),
                    # configuration (10), then the reference year, month, day.
                    y, m, d = struct.unpack(">HBB", body[35:39])
                    day = datetime(y, m, d, tzinfo=timezone.utc)
                elif kind == DWELL and day and len(body) >= 19:
                    ms = struct.unpack(">I", body[15:19])[0]
                    t = (day + timedelta(milliseconds=ms)).timestamp()
                j += max(seg, SEGMENT_HEADER)
            yield t, pkt
            i += size


def device(path):
    """A name for a GPS log: `13-OCT-15 TM01.gpx` → TM01, `31_1018.gpx` → GPS31."""
    stem = path.stem
    m = re.search(r"(TM\d+|KAYAK)$", stem.upper())
    if m:
        return m.group(1)
    m = re.match(r"(\d+)_\d+$", stem)
    return f"GPS{m.group(1)}" if m else re.sub(r"\W+", "-", stem)


def gpx_points(path):
    s = path.read_text(encoding="utf-8-sig", errors="replace")
    out = []
    for attrs, body in PT.findall(s):
        a = dict(ATTR.findall(attrs))
        m = TIME.search(body)
        if m and "lat" in a and "lon" in a:
            t = datetime.fromisoformat(m.group(1).replace("Z", "+00:00")).timestamp()
            out.append((t, float(a["lat"]), float(a["lon"])))
    out.sort()
    return out


def motion(pts, i):
    """Course (deg) and speed (m/s) from the points a few seconds either side."""
    lo, hi = i, i
    while lo > 0 and pts[i][0] - pts[lo - 1][0] <= 3:
        lo -= 1
    while hi < len(pts) - 1 and pts[hi + 1][0] - pts[i][0] <= 3:
        hi += 1
    (t0, a0, o0), (t1, a1, o1) = pts[lo], pts[hi]
    if t1 - t0 < 0.5:
        return None, None
    dn = (a1 - a0) * 110540
    de = (o1 - o0) * 111320 * math.cos(math.radians(a0))
    speed = math.hypot(dn, de) / (t1 - t0)
    course = math.degrees(math.atan2(de, dn)) % 360 if speed > 0.5 else None
    return course, round(speed, 2)


def iso(t):
    return datetime.fromtimestamp(t, timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--gmti", nargs="+", required=True, type=Path)
    ap.add_argument("--gpx", nargs="+", required=True, type=Path)
    ap.add_argument("--port", type=int, default=4607)
    ap.add_argument("--bind", default="127.0.0.1")
    ap.add_argument("--gps", default="127.0.0.1:47010", help="host:port of the GPS source (UDP)")
    ap.add_argument("--speed", type=float, default=1.0, help="multiple of real time")
    ap.add_argument("--loops", type=int, default=0, help="0: forever")
    args = ap.parse_args()

    packets = list(gmti_packets(sorted(args.gmti, key=lambda p: p.name)))
    times = [t for t, _ in packets if t is not None]
    if not times:
        raise SystemExit("no dwells (and a mission segment before them) in the GMTI files")
    start, end = min(times), max(times)
    gps = []
    for path in args.gpx:
        pts = [p for p in gpx_points(path) if start <= p[0] <= end]
        if len(pts) < 2:
            print(f"{path.name}: no points between {iso(start)} and {iso(end)}; skipped")
            continue
        name = device(path)
        domain = "surface" if name == "KAYAK" else "ground"
        for i, (t, lat, lon) in enumerate(pts):
            course, speed = motion(pts, i)
            gps.append((t, name, {"lat": lat, "lon": lon, "course": course, "speed": speed, "domain": domain}))
        print(f"{name}: {len(pts)} points {iso(pts[0][0])} to {iso(pts[-1][0])}")
    gps.sort(key=lambda g: g[0])
    last_of = {}
    for t, name, _ in gps:
        last_of[name] = t

    # One timeline: GMTI packets (undated ones go with the dated packet after
    # them) and GPS points.
    events, pending = [], []
    for t, pkt in packets:
        pending.append(pkt)
        if t is not None:
            events.append((t, 0, pending))
            pending = []
    if pending:
        events.append((end, 0, pending))
    events += [(t, 1, (name, p)) for t, name, p in gps]
    events.sort(key=lambda e: (e[0], e[1]))
    print(f"{len(packets)} GMTI packets and {len(gps)} GPS points over {end - start:.0f} s "
          f"({iso(start)} to {iso(end)}); waiting for the GMTI source on {args.bind}:{args.port}")

    host, port = args.gps.rsplit(":", 1)
    udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    srv = socket.create_server((args.bind, args.port))
    loop = 0
    while args.loops == 0 or loop < args.loops:
        conn, addr = srv.accept()
        print(f"GMTI source connected from {addr[0]}:{addr[1]}", flush=True)
        try:
            while args.loops == 0 or loop < args.loops:
                loop += 1
                wall0 = time.time()
                shift = lambda t: wall0 + (t - start) / args.speed  # noqa: E731
                print(f"loop {loop}: recording {iso(start)} is now {iso(wall0)}", flush=True)
                for t, kind, what in events:
                    wait = shift(t) - time.time()
                    if wait > 0:
                        time.sleep(wait)
                    if kind == 0:
                        for pkt in what:
                            conn.sendall(pkt)
                        continue
                    name, p = what
                    msg = dict(p, id=f"{name}-r{loop}", name=name, t=iso(shift(t)))
                    if t == last_of[name]:
                        msg["state"] = "dropped"
                    udp.sendto(json.dumps(msg).encode(), (host, int(port)))
                print(f"loop {loop} done", flush=True)
        except (BrokenPipeError, ConnectionResetError):
            print("GMTI source disconnected; waiting for it again", flush=True)
        finally:
            conn.close()


if __name__ == "__main__":
    main()

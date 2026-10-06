#!/usr/bin/env python3
"""Replay Autoferry scenarios into a running OpenTrack, in real time, as if
happening now: the vessel track feed and the lidar and radar detections go to
the demo sources in docs/examples/autoferry (UDP on localhost).

Each loop renames the vessel keys (`r2-target-1`), so every run makes fresh
tracks, and ends its vessel tracks (`state: dropped`) when it finishes; the
trackers end their own. Everything the demo sources publish is marked
simulated training.

Usage: autoferry-replay.py [--loops N] [--scenarios 2 16] [--host 127.0.0.1]
Needs the fixtures from scripts/benchmark/replay/autoferry.py (crates/ot-server/tests/data/autoferry).
"""

import argparse
import json
import socket
import time
from datetime import datetime, timezone
from pathlib import Path

FIXTURES = Path(__file__).resolve().parents[3] / "crates/ot-server/tests/data/autoferry"
PORTS = {"track": 47001, "lidar-det": 47002, "radar-det": 47003}
GAP_S = 20.0


def ts(s):
    return datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp()


def iso(t):
    return datetime.fromtimestamp(t, timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


def frames(scenario, run):
    """(time, feed, payload) in time order; each sensor scan is one frame."""
    rows = [json.loads(line) for line in open(FIXTURES / f"scenario{scenario}-detections.jsonl")]
    out, scans = [], {}
    for r in rows:
        o = r["obs"]
        t = ts(o["observed_at"])
        base = {
            "lat": o["position"]["latitude"],
            "lon": o["position"]["longitude"],
            "cep": (o.get("uncertainty") or {}).get("circular_error_m"),
        }
        if r["feed"] == "track":
            k = o.get("kinematics", {})
            out.append((t, "track", dict(base, id=f"r{run}-{o['source_track_key']}", name=o.get("name"),
                                         course=k.get("course_deg"), speed=k.get("speed_mps"))))
        else:
            scans.setdefault((r["feed"], t), []).append(dict(base, id=o["source_track_key"]))
    for (feed, t), plots in scans.items():
        out.append((t, feed, {"plots": plots}))
    out.sort(key=lambda f: f[0])
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--loops", type=int, default=0, help="0: forever")
    ap.add_argument("--scenarios", type=int, nargs="+", default=[2, 16])
    ap.add_argument("--host", default="127.0.0.1")
    args = ap.parse_args()
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    run = 0
    while args.loops == 0 or run < args.loops * len(args.scenarios):
        scenario = args.scenarios[run % len(args.scenarios)]
        run += 1
        fs = frames(scenario, run)
        t0, start = fs[0][0], time.time()
        last = {}
        print(f"run {run}: scenario {scenario}, {len(fs)} frames over {fs[-1][0] - t0:.0f} s", flush=True)
        for t, feed, payload in fs:
            due = start + (t - t0)
            wait = due - time.time()
            if wait > 0:
                time.sleep(wait)
            now = iso(due)
            if feed == "track":
                payload["t"] = now
                last[payload["id"]] = payload
            else:
                for p in payload["plots"]:
                    p["t"] = now
            sock.sendto(json.dumps(payload).encode(), (args.host, PORTS[feed]))
        # The vessels' feed says their tracks are over.
        for payload in last.values():
            payload.update(t=iso(time.time()), state="dropped")
            sock.sendto(json.dumps(payload).encode(), (args.host, PORTS["track"]))
        time.sleep(GAP_S)


if __name__ == "__main__":
    main()

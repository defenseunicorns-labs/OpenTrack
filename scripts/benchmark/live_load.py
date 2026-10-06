"""Sustained live load: the whole server (sources, engine, writer) against a
Redis and a NATS of its own, fed N tracks at 1 Hz over UDP for as long as
you like, measured as it runs.

    scripts/benchmark/bench live --tracks 16000 --minutes 10

It starts throwaway Redis and NATS containers (never the live ones), runs
`target/release/opentrack all` with sign-in off, adds a UDP source through
the API, and feeds it from here. Every 10 s it samples the server's
metrics (engine and writer backlogs, live tracks, memory, CPU, Redis) and
a NATS subscriber's view of what was published (messages a second, and the
delay from a report's time to its publication, which includes the writer's
5 s per-track coalescing). Results go to $OT_BENCH_DATA/results/live-*.
"""

import asyncio
import json
import math
import os
import random
import socket
import statistics
import subprocess
import sys
import time
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

from common import REPO, data_dir

REDIS_PORT, NATS_PORT, API_PORT, UDP_PORT = 6397, 4227, 18097, 47197
PER_DATAGRAM = 40


def http(method, path, body=None):
    req = urllib.request.Request(
        f"http://127.0.0.1:{API_PORT}/api/v1{path}", method=method,
        data=None if body is None else json.dumps(body).encode(),
        headers={"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=30) as r:
        raw = r.read()
        return json.loads(raw) if raw else None


def docker(*args):
    return subprocess.run(["docker", *args], capture_output=True, text=True)


class Targets:
    """N targets moving at constant course and speed, spread over the Atlantic."""

    def __init__(self, n, seed=7):
        rng = random.Random(seed)
        self.t = [(f"{300000000 + k}", rng.uniform(30, 55), rng.uniform(-60, -10),
                   rng.uniform(0, 360), rng.uniform(2, 15)) for k in range(n)]
        self.start = time.time()

    def datagrams(self, now):
        s = now - self.start
        stamp = datetime.fromtimestamp(now, timezone.utc).isoformat().replace("+00:00", "Z")
        recs = []
        for mmsi, lat, lon, course, speed in self.t:
            d = speed * s
            r = math.radians(course)
            recs.append({"id": mmsi, "t": stamp,
                         "lat": round(lat + math.degrees(d * math.cos(r) / 6378137.0), 6),
                         "lon": round(lon + math.degrees(d * math.sin(r) / (6378137.0 * math.cos(math.radians(lat)))), 6),
                         "course": round(course, 1), "speed": round(speed, 2), "cep": 10.0,
                         "domain": "surface"})
        for i in range(0, len(recs), PER_DATAGRAM):
            yield json.dumps({"reports": recs[i:i + PER_DATAGRAM]}).encode()


def feed(targets, seconds, sent):
    """Send every target once a second, spread over the second."""
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, 8 << 20)
    begin = time.time()
    for s in range(seconds):
        tick = begin + s
        grams = list(targets.datagrams(tick))
        for i, g in enumerate(grams):
            due = tick + i / len(grams)
            wait = due - time.time()
            if wait > 0:
                time.sleep(wait)
            sock.sendto(g, ("127.0.0.1", UDP_PORT))
        sent[0] += len(targets.t)
        if s % 60 == 59:
            print(f"  fed {s + 1} s", flush=True)


async def subscribe(stop, seen):
    import nats
    for _ in range(50):
        try:
            nc = await nats.connect(f"nats://127.0.0.1:{NATS_PORT}", allow_reconnect=False)
            break
        except Exception:
            await asyncio.sleep(0.2)
    else:
        raise SystemExit("could not reach the load test's NATS")

    async def on(msg):
        now = time.time()
        try:
            doc = json.loads(msg.data)
            t = datetime.fromisoformat(doc["time"].replace("Z", "+00:00")).timestamp()
            seen.append((now, now - t))
        except Exception:
            pass

    await nc.subscribe("loadtest.>", cb=on)
    while not stop.is_set():
        await asyncio.sleep(0.2)
    await nc.drain()


def sample():
    m = http("GET", "/metrics?minutes=2")
    live = m["live"]
    out = {k: live.get(k) for k in ("tracks", "rss_bytes", "redis_bytes", "cpu_milli", "nats_messages")}
    for group, name in (("observations", "obs"), ("outbox", "outbox")):
        b = live.get(group) or {}
        out[f"{name}_lag"] = b.get("lag", 0)
        out[f"{name}_pending"] = b.get("pending", 0)
    return out


def main(argv):
    import argparse
    ap = argparse.ArgumentParser(prog="bench live")
    ap.add_argument("--tracks", type=int, default=16000)
    ap.add_argument("--minutes", type=float, default=10)
    ap.add_argument("--label", default="")
    ap.add_argument("--no-build", action="store_true")
    a = ap.parse_args(argv)
    exe = REPO / "target/release/opentrack"
    if not a.no_build:
        subprocess.run(["cargo", "build", "--release", "-q", "-p", "ot-server"], cwd=REPO, check=True)
    run = f"live-{datetime.now():%Y%m%d-%H%M%S}-{a.tracks // 1000}k" + (f"-{a.label}" if a.label else "")
    out = data_dir() / "results" / run
    out.mkdir(parents=True, exist_ok=True)
    work = out / "work"
    work.mkdir()

    docker("rm", "-f", "otload-redis", "otload-nats")
    docker("run", "-d", "--rm", "--name", "otload-redis", "-p", f"127.0.0.1:{REDIS_PORT}:6379", "redis:7")
    docker("run", "-d", "--rm", "--name", "otload-nats", "-p", f"127.0.0.1:{NATS_PORT}:4222", "nats:2", "-js")
    time.sleep(2)
    env = {"PATH": "/usr/bin:/bin", "OT_AUTH": "off", "OT_LOG": "warn",
           "OT_SQLITE_PATH": str(work / "ot.db"), "OT_REDIS_URL": f"redis://127.0.0.1:{REDIS_PORT}",
           "OT_NATS_URL": f"nats://127.0.0.1:{NATS_PORT}", "OT_NATS_STREAM": "LOADTEST",
           "OT_NATS_TRACKS_SUBJECT": "loadtest", "OT_BIND": f"127.0.0.1:{API_PORT}", "OT_UI_DIR": "/nonexistent"}
    log = open(out / "server.log", "w")
    server = subprocess.Popen([str(exe), "all"], env=env, stdout=log, stderr=log)
    try:
        for _ in range(50):
            try:
                http("GET", "/status")
                break
            except Exception:
                time.sleep(0.2)
        http("PUT", "/schema/draft", json.loads((REPO / "docs/examples/schema.json").read_text()))
        http("POST", "/schema/draft/publish")
        spec = json.loads((data_dir() / "scenarios/load-2k/scenario.json").read_text())["sources"][0]["spec"]
        spec.update(id="load", name=f"Live load: {a.tracks} tracks at 1 Hz")
        spec["transport"] = {"type": "udp", "bind": f"127.0.0.1:{UDP_PORT}"}
        # A loopback UDP feed: its (lack of) sender authentication is accepted.
        spec["unauthenticated"] = "accepted"
        http("POST", "/sources", spec)
        http("POST", "/sources/load/enable")
        time.sleep(3)

        seconds = int(a.minutes * 60)
        targets = Targets(a.tracks)
        sent, seen, samples = [0], [], []
        stop = asyncio.Event()
        loop = asyncio.new_event_loop()
        import threading
        sub = threading.Thread(target=lambda: loop.run_until_complete(subscribe(stop, seen)), daemon=True)
        sub.start()
        feeder = threading.Thread(target=feed, args=(targets, seconds, sent), daemon=True)
        print(f"{run}: {a.tracks} tracks at 1 Hz for {a.minutes:g} min")
        t0 = time.time()
        feeder.start()
        print(f"{'t':>5} {'tracks':>7} {'obs lag':>8} {'out lag':>8} {'pub/s':>7} {'delay p50':>9} {'p95':>6} "
              f"{'rss MB':>7} {'redis MB':>8} {'cpu %':>6}")
        last_seen = 0
        while feeder.is_alive() or time.time() - t0 < seconds + 15:
            time.sleep(10)
            s = sample()
            s["t"] = round(time.time() - t0)
            window = seen[last_seen:]
            last_seen = len(seen)
            s["published_per_s"] = round(len(window) / 10)
            delays = sorted(d for _, d in window)
            s["delay_p50"] = round(delays[len(delays) // 2], 2) if delays else None
            s["delay_p95"] = round(delays[int(len(delays) * 0.95)], 2) if delays else None
            samples.append(s)
            mb = lambda b: round((b or 0) / 1e6)
            print(f"{s['t']:>5} {s['tracks']:>7} {s['obs_lag']:>8} {s['outbox_lag']:>8} {s['published_per_s']:>7} "
                  f"{s['delay_p50'] if s['delay_p50'] is not None else '-':>9} {s['delay_p95'] if s['delay_p95'] is not None else '-':>6} "
                  f"{mb(s['rss_bytes']):>7} {mb(s['redis_bytes']):>8} {(s['cpu_milli'] or 0) / 10:>6.0f}", flush=True)
            if not feeder.is_alive() and time.time() - t0 > seconds + 15:
                break
        stop.set()
        steady = [s for s in samples if s["t"] > 60 and s["t"] <= seconds]
        delays = sorted(d for at, d in seen if at - t0 > 60)
        q = lambda p: round(delays[min(len(delays) - 1, int(len(delays) * p))], 2) if delays else None
        summary = {
            "run": run, "tracks": a.tracks, "seconds": seconds, "fed": sent[0],
            "fed_per_s": round(sent[0] / max(1, seconds)),
            "published": len(seen),
            "published_per_s": round(statistics.mean(s["published_per_s"] for s in steady)) if steady else None,
            "delay_p50_s": q(0.5), "delay_p95_s": q(0.95), "delay_p99_s": q(0.99),
            "max_obs_lag": max((s["obs_lag"] or 0) for s in steady) if steady else None,
            "max_outbox_lag": max((s["outbox_lag"] or 0) for s in steady) if steady else None,
            "final_rss_mb": round((samples[-1]["rss_bytes"] or 0) / 1e6) if samples else None,
            "final_redis_mb": round((samples[-1]["redis_bytes"] or 0) / 1e6) if samples else None,
            "mean_cpu_pct": round(statistics.mean((s["cpu_milli"] or 0) / 10 for s in steady)) if steady else None,
            "live_tracks": samples[-1]["tracks"] if samples else None,
        }
        (out / "samples.json").write_text(json.dumps(samples, indent=1) + "\n")
        (out / "summary.json").write_text(json.dumps(summary, indent=1) + "\n")
        print(json.dumps(summary, indent=1))
        print(out)
    finally:
        server.terminate()
        try:
            server.wait(10)
        except subprocess.TimeoutExpired:
            server.kill()
        docker("rm", "-f", "otload-redis", "otload-nats")


if __name__ == "__main__":
    sys.path.insert(0, str(Path(__file__).parent))
    main(sys.argv[1:])

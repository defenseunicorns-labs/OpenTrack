"""Several OpenTrack nodes sharing one picture, measured against the truth.

    scripts/benchmark/bench swarm --nodes 4 --targets 300 --minutes 5
    scripts/benchmark/bench swarm --loss 0.2 --rate-kbps 64 --partition 120-240

Each node is a whole OpenTrack (`opentrack all`, sign-in off) with a sensor of
its own: a circle of the sea it sees. Even nodes see like AIS (identities,
10 m); odd nodes like a radar tracker (no identities, ~50 m noise, its own
track numbers). Targets sail straight through a 70 km box. The nodes'
sync messages go through `opentrack bridge`, which stands in for the
networking package: it can lose, delay, duplicate, cap and partition.

Every 10 s it compares each node's published picture (its NATS output)
with the truth:

* coverage: of the targets some node sees, the share each node holds;
* one number: the share of seen targets that every node holds under the
  same track number;
* duplicates: extra tracks on one target on one node (dual designations);
* bandwidth: track reports each node sends, kbit/s (from the bridge).

With `--partition <from>-<to>` the first half of the nodes is cut off from
the second for that time (seconds from the start); the run reports how long
the picture takes to be one again after the cut heals. Results go to
$OT_BENCH_DATA/results/swarm-*.
"""

import asyncio
import json
import math
import random
import socket
import statistics
import subprocess
import sys
import threading
import time
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

from common import REPO, data_dir

REDIS_PORT, NATS_PORT = 6396, 4228
API_BASE, UDP_BASE = 18110, 47210
BOX = (50.0, 50.6, -1.6, -0.6)  # lat min/max, lon min/max
MATCH_M = 1000.0
R = 6371008.8


def dist_m(a, b):
    x = math.radians(b[1] - a[1]) * math.cos(math.radians((a[0] + b[0]) / 2))
    y = math.radians(b[0] - a[0])
    return R * math.hypot(x, y)


def docker(*args):
    return subprocess.run(["docker", *args], capture_output=True, text=True)


def http(port, method, path, body=None):
    req = urllib.request.Request(
        f"http://127.0.0.1:{port}/api/v1{path}", method=method,
        data=None if body is None else json.dumps(body).encode(),
        headers={"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=30) as r:
        raw = r.read()
        return json.loads(raw) if raw else None


class World:
    """Targets on straight courses, bouncing off the box's edges."""

    def __init__(self, n, seed):
        rng = random.Random(seed)
        self.t0 = time.time()
        self.targets = []
        for k in range(n):
            self.targets.append({
                "mmsi": str(235000000 + k),
                "lat": rng.uniform(BOX[0], BOX[1]), "lon": rng.uniform(BOX[2], BOX[3]),
                "course": rng.uniform(0, 360), "speed": rng.uniform(3, 15),
            })

    def step(self, dt):
        for t in self.targets:
            d = t["speed"] * dt
            c = math.radians(t["course"])
            t["lat"] += math.degrees(d * math.cos(c) / R)
            t["lon"] += math.degrees(d * math.sin(c) / (R * math.cos(math.radians(t["lat"]))))
            if not BOX[0] < t["lat"] < BOX[1]:
                t["course"] = (180 - t["course"]) % 360
                t["lat"] = min(max(t["lat"], BOX[0]), BOX[1])
            if not BOX[2] < t["lon"] < BOX[3]:
                t["course"] = (-t["course"]) % 360
                t["lon"] = min(max(t["lon"], BOX[2]), BOX[3])


class Node:
    def __init__(self, i, n, radius_m, rng):
        self.i = i
        self.site = f"N{i + 1:02d}"
        self.api = API_BASE + i
        self.udp = UDP_BASE + i
        self.ais = i % 2 == 0
        # Sensors on a ring inside the box, so neighbours overlap.
        a = 2 * math.pi * i / n
        self.centre = ((BOX[0] + BOX[1]) / 2 + 0.18 * math.sin(a), (BOX[2] + BOX[3]) / 2 + 0.28 * math.cos(a))
        self.radius = radius_m
        self.keys = {}
        self.rng = rng
        self.proc = None

    def sees(self, t):
        return dist_m(self.centre, (t["lat"], t["lon"])) <= self.radius

    def records(self, world, stamp):
        out = []
        for t in world.targets:
            if not self.sees(t):
                continue
            if self.ais:
                out.append({"id": t["mmsi"], "t": stamp, "lat": round(t["lat"], 6), "lon": round(t["lon"], 6),
                            "course": round(t["course"], 1), "speed": round(t["speed"], 2), "cep": 10.0,
                            "domain": "surface"})
            else:
                key = self.keys.setdefault(t["mmsi"], f"{self.site}-{self.rng.randrange(10**6):06d}")
                s = 50.0 / 1.1774
                n, e = self.rng.gauss(0, s), self.rng.gauss(0, s)
                out.append({"id": key, "t": stamp,
                             "lat": round(t["lat"] + math.degrees(n / R), 6),
                             "lon": round(t["lon"] + math.degrees(e / (R * math.cos(math.radians(t["lat"])))), 6),
                             "course": round((t["course"] + self.rng.gauss(0, 3)) % 360, 1),
                             "speed": round(max(0.0, t["speed"] + self.rng.gauss(0, 0.5)), 2), "cep": 50.0,
                             "domain": "surface"})
        return out


def feed(world, nodes, seconds, stop):
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, 8 << 20)
    begin = time.time()
    for s in range(seconds):
        if stop.is_set():
            return
        tick = begin + s
        wait = tick - time.time()
        if wait > 0:
            time.sleep(wait)
        world.step(1.0 if s else 0.0)
        stamp = datetime.fromtimestamp(time.time(), timezone.utc).isoformat().replace("+00:00", "Z")
        for n in nodes:
            recs = n.records(world, stamp)
            for i in range(0, len(recs), 40):
                sock.sendto(json.dumps({"reports": recs[i:i + 40]}).encode(), ("127.0.0.1", n.udp))


class Pictures:
    """Every node's published tracks: uid -> (lat, lon), per node."""

    def __init__(self, sites):
        self.by = {s: {} for s in sites}
        self.lock = threading.Lock()

    async def run(self, stop):
        import nats
        for _ in range(50):
            try:
                nc = await nats.connect(f"nats://127.0.0.1:{NATS_PORT}", allow_reconnect=False)
                break
            except Exception:
                await asyncio.sleep(0.2)
        else:
            raise SystemExit("could not reach the swarm's NATS")

        async def on(msg):
            site = msg.subject.split(".")[1]
            try:
                doc = json.loads(msg.data)
            except Exception:
                return
            with self.lock:
                pic = self.by.get(site)
                if pic is None:
                    return
                if doc.get("op") == "delete":
                    pic.pop(doc["uid"], None)
                elif "lat" in doc:
                    pic[doc["uid"]] = (doc["lat"], doc["lon"])

        await nc.subscribe("sw.>", cb=on)
        while not stop.is_set():
            await asyncio.sleep(0.2)
        await nc.drain()

    def snapshot(self):
        with self.lock:
            return {s: dict(p) for s, p in self.by.items()}


def score(world, nodes, pics, misses=None):
    """Each track goes to its nearest seen target (within MATCH_M); a target
    with more than one track on a node has duplicates there."""
    seen = [t for t in world.targets if any(n.sees(t) for n in nodes)]
    pos = [(t["lat"], t["lon"]) for t in seen]
    held = one = dup = 0
    per_node = []
    for n in nodes:
        got = [[] for _ in seen]
        for u, p in pics[n.site].items():
            best = min(((dist_m(q, p), i) for i, q in enumerate(pos)), default=None)
            if best and best[0] <= MATCH_M:
                got[best[1]].append((best[0], u))
        per_node.append(got)
    for i in range(len(seen)):
        uids = []
        for got in per_node:
            tracks = sorted(got[i])
            if tracks:
                held += 1
                uids.append(tracks[0][1])
                dup += len(tracks) - 1
            else:
                uids.append(None)
        doubled = any(len(got[i]) > 1 for got in per_node)
        if None not in uids and len(set(uids)) == 1:
            one += 1
        if misses is not None and (doubled or None in uids or len(set(uids)) > 1):
            t = seen[i]
            misses.append({"target": t["mmsi"], "lat": round(t["lat"], 5), "lon": round(t["lon"], 5),
                           "seen_by": [n.site for n in nodes if n.sees(t)],
                           "tracks": {n.site: [[u, round(d)] for d, u in sorted(per_node[j][i])] for j, n in enumerate(nodes)}})
    k = max(1, len(seen))
    pairs = max(1, len(seen) * len(nodes))
    return {"seen": len(seen), "coverage": held / pairs, "one_number": one / k, "duplicates": dup / pairs}


def main(argv):
    import argparse
    ap = argparse.ArgumentParser(prog="bench swarm")
    ap.add_argument("--nodes", type=int, default=4)
    ap.add_argument("--targets", type=int, default=300)
    ap.add_argument("--minutes", type=float, default=5)
    ap.add_argument("--radius-km", type=float, default=22.0)
    ap.add_argument("--loss", type=float, default=0.0)
    ap.add_argument("--delay-ms", type=int, default=0)
    ap.add_argument("--jitter-ms", type=int, default=0)
    ap.add_argument("--duplicate", type=float, default=0.0)
    ap.add_argument("--rate-kbps", type=float, default=0.0, help="the link's cap per node (the bridge drops past it)")
    ap.add_argument("--budget-kbps", type=float, default=0.0, help="each node's own sending budget (Settings)")
    ap.add_argument("--partition", help="<from>-<to> seconds: the first half of the nodes cut off from the rest")
    ap.add_argument("--seed", type=int, default=7)
    ap.add_argument("--label", default="")
    ap.add_argument("--no-build", action="store_true")
    a = ap.parse_args(argv)
    exe = REPO / "target/release/opentrack"
    if not a.no_build:
        subprocess.run(["cargo", "build", "--release", "-q", "-p", "ot-server"], cwd=REPO, check=True)
    run = f"swarm-{datetime.now():%Y%m%d-%H%M%S}-{a.nodes}n" + (f"-{a.label}" if a.label else "")
    out = data_dir() / "results" / run
    work = out / "work"
    work.mkdir(parents=True)
    rng = random.Random(a.seed)
    nodes = [Node(i, a.nodes, a.radius_km * 1000, random.Random(a.seed * 100 + i)) for i in range(a.nodes)]
    sites = [n.site for n in nodes]

    docker("rm", "-f", "otswarm-redis", "otswarm-nats")
    docker("run", "-d", "--rm", "--name", "otswarm-redis", "-p", f"127.0.0.1:{REDIS_PORT}:6379", "redis:7")
    docker("run", "-d", "--rm", "--name", "otswarm-nats", "-p", f"127.0.0.1:{NATS_PORT}:4222", "nats:2", "-js")
    time.sleep(2)
    procs = []
    stop = threading.Event()
    try:
        spec0 = json.loads((data_dir() / "scenarios/load-2k/scenario.json").read_text())["sources"][0]["spec"]
        for n in nodes:
            env = {"PATH": "/usr/bin:/bin", "OT_AUTH": "off", "OT_LOG": "warn", "OT_SITE_CODE": n.site,
                   "OT_SQLITE_PATH": str(work / f"{n.site}.db"),
                   "OT_REDIS_URL": f"redis://127.0.0.1:{REDIS_PORT}", "OT_REDIS_NAMESPACE": f"sw-{n.site}",
                   "OT_NATS_URL": f"nats://127.0.0.1:{NATS_PORT}", "OT_NATS_STREAM": f"SW_{n.site}",
                   "OT_NATS_TRACKS_SUBJECT": f"sw.{n.site}", "OT_SYNC_PREFIX": f"ot.sync.{n.site}",
                   "OT_BIND": f"127.0.0.1:{n.api}", "OT_UI_DIR": "/nonexistent"}
            log = open(out / f"{n.site}.log", "w")
            n.proc = subprocess.Popen([str(exe), "all"], env=env, stdout=log, stderr=log)
            procs.append(n.proc)
        bridge_cmd = [str(exe), "bridge", "--stats", str(out / "bridge.json"), "--seed", str(a.seed),
                      "--loss", str(a.loss), "--delay-ms", str(a.delay_ms), "--jitter-ms", str(a.jitter_ms),
                      "--duplicate", str(a.duplicate), "--rate-kbps", str(a.rate_kbps)]
        for n in nodes:
            bridge_cmd += ["--node", f"nats://127.0.0.1:{NATS_PORT}#ot.sync.{n.site}"]
        if a.partition:
            half = a.nodes // 2
            bridge_cmd += ["--partition", f"{a.partition}:{','.join(sites[:half])}|{','.join(sites[half:])}"]
        for n in nodes:
            for _ in range(100):
                try:
                    http(n.api, "GET", "/status")
                    break
                except Exception:
                    time.sleep(0.2)
            http(n.api, "PUT", "/schema/draft", json.loads((REPO / "docs/examples/schema.json").read_text()))
            http(n.api, "POST", "/schema/draft/publish")
            spec = json.loads(json.dumps(spec0))
            spec.update(id="sensor", name=f"{'AIS' if n.ais else 'Radar tracker'} of {n.site}")
            spec["transport"] = {"type": "udp", "bind": f"127.0.0.1:{n.udp}"}
            # A loopback UDP feed: its (lack of) sender authentication is accepted.
            spec["unauthenticated"] = "accepted"
            if not n.ais:
                spec["pipeline"]["mapping"]["rules"][0]["identifiers"] = []
                spec["publish_alone"] = True
            http(n.api, "POST", "/sources", spec)
            http(n.api, "POST", "/sources/sensor/enable")
            http(n.api, "PUT", "/settings", {"sync": {"enabled": True, "peers": [s for s in sites if s != n.site],
                                                      "budget_kbps": a.budget_kbps}})
        # Every node accepts only messages signed with a key pinned for the
        # sender (Settings -> Nodes): pin each node's key on the others.
        keys = {n.site: http(n.api, "GET", "/sync/keys")["public_key"] for n in nodes}
        for n in nodes:
            for site, key in keys.items():
                if site != n.site:
                    http(n.api, "PUT", f"/sync/keys/{site}", {"public_key": key})
        blog = open(out / "bridge.log", "w")
        bridge = subprocess.Popen(bridge_cmd, env={"PATH": "/usr/bin:/bin", "OT_LOG": "info"}, stdout=blog, stderr=blog)
        procs.append(bridge)
        time.sleep(6)  # settings reach every role

        world = World(a.targets, a.seed)
        seconds = int(a.minutes * 60)
        pics = Pictures(sites)
        loop = asyncio.new_event_loop()
        threading.Thread(target=lambda: loop.run_until_complete(pics.run(stop)), daemon=True).start()
        feeder = threading.Thread(target=feed, args=(world, nodes, seconds, stop), daemon=True)
        t0 = time.time()
        feeder.start()
        print(f"{run}: {a.nodes} nodes, {a.targets} targets, {a.minutes:g} min"
              + (f", loss {a.loss:g}" if a.loss else "") + (f", {a.rate_kbps:g} kbit/s" if a.rate_kbps else "")
              + (f", partition {a.partition} s" if a.partition else ""))
        print(f"{'t':>5} {'seen':>5} {'coverage':>9} {'one no.':>8} {'dups':>6} {'kbit/s/node':>12}")
        samples, last_bytes, last_t = [], {}, t0
        while feeder.is_alive():
            time.sleep(10)
            now = time.time()
            misses = []
            s = score(world, nodes, pics.snapshot(), misses)
            s["t"] = round(now - t0)
            with open(out / "misses.jsonl", "a") as f:
                f.write(json.dumps({"t": s["t"], "misses": misses}) + "\n")
            try:
                stats = json.loads((out / "bridge.json").read_text())["nodes"]
            except Exception:
                stats = {}
            rates = []
            for site in sites:
                b = stats.get(site, {}).get("report_bytes", 0)
                rates.append((b - last_bytes.get(site, 0)) * 8 / 1000 / max(1e-3, now - last_t))
                last_bytes[site] = b
            last_t = now
            s["report_kbps_per_node"] = round(statistics.mean(rates), 2) if rates else 0
            samples.append(s)
            print(f"{s['t']:>5} {s['seen']:>5} {s['coverage']:>9.1%} {s['one_number']:>8.1%} {s['duplicates']:>6.1%} "
                  f"{s['report_kbps_per_node']:>12.2f}", flush=True)
        stop.set()

        warm = [s for s in samples if s["t"] > 60]
        cut = None
        if a.partition:
            f, t = (float(x) for x in a.partition.split("-"))
            cut = (f, t)
            steady = [s for s in warm if s["t"] < f or s["t"] > t + 60]
        else:
            steady = warm
        mean = lambda k, xs: round(statistics.mean(x[k] for x in xs), 4) if xs else None
        summary = {
            "run": run, "nodes": a.nodes, "targets": a.targets, "seconds": seconds,
            "loss": a.loss, "delay_ms": a.delay_ms, "jitter_ms": a.jitter_ms, "duplicate": a.duplicate,
            "rate_kbps": a.rate_kbps, "budget_kbps": a.budget_kbps, "partition": a.partition,
            "seen_mean": mean("seen", steady),
            "coverage": mean("coverage", steady), "one_number": mean("one_number", steady),
            "duplicates": mean("duplicates", steady),
            "report_kbps_per_node": mean("report_kbps_per_node", steady),
        }
        if summary["seen_mean"]:
            summary["report_kbps_per_node_per_500_tracks"] = round(
                summary["report_kbps_per_node"] * 500 / summary["seen_mean"], 2)
        if cut:
            before = [s["one_number"] for s in warm if s["t"] < cut[0]]
            level = (statistics.mean(before) if before else 1.0) - 0.01
            healed = [s for s in samples if s["t"] >= cut[1] and s["one_number"] >= level]
            summary["converged_after_heal_s"] = (healed[0]["t"] - cut[1]) if healed else None
            during = [s for s in samples if cut[0] + 20 <= s["t"] <= cut[1]]
            summary["one_number_during_partition"] = mean("one_number", during)
        (out / "samples.json").write_text(json.dumps(samples, indent=1) + "\n")
        (out / "summary.json").write_text(json.dumps(summary, indent=1) + "\n")
        print(json.dumps(summary, indent=1))
        print(out)
    finally:
        stop.set()
        for p in procs:
            p.terminate()
        for p in procs:
            try:
                p.wait(10)
            except subprocess.TimeoutExpired:
                p.kill()
        docker("rm", "-f", "otswarm-redis", "otswarm-nats")


if __name__ == "__main__":
    sys.path.insert(0, str(Path(__file__).parent))
    main(sys.argv[1:])

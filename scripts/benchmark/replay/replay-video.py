#!/usr/bin/env python3
"""Render an engine replay trace as a video: reports and detections as they
arrive, coloured by the system track the engine put them on, with pairings.

Traces come from the Autoferry replay tests:
    OT_REPLAY_TRACE=<dir> cargo test -p ot-server autoferry

Usage: replay-video.py <trace.jsonl> <fixture.jsonl> <out.mp4> [--dataset <autoferry dir>]
Needs matplotlib and imageio-ffmpeg. With --dataset, the ferry (milliAmpere)
and the true target positions are drawn too.
"""

import argparse
import json
import math
from datetime import datetime
from pathlib import Path

import imageio_ffmpeg
import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

PIREN = (63.4389029083, 10.39908278)
A, F = 6378137.0, 1 / 298.257223563
E2 = F * (2 - F)
FPS, STEP = 20, 0.25  # 20 frames per second, 0.25 s of scenario each: 5x speed
TRAIL_S, FADE_S = 12.0, 2.5

BG, FG, MUTED, GRID = "#0f1419", "#e6e8eb", "#6b7580", "#1f2730"
TARGET_COLOURS = {1: "#f5a524", 2: "#3fb8f0"}
OTHER = ["#b07cf2", "#6fd08c", "#f06b8a", "#c9c26b", "#8a9bf5", "#e0895c", "#5cc9c0", "#d68ad6"]
MARKERS = {"track": "s", "lidar": "o", "radar": "^", "lidar-det": ".", "radar-det": "."}


def local(lat, lon):
    lat0 = math.radians(PIREN[0])
    s = math.sin(lat0)
    rn = A / math.sqrt(1 - E2 * s * s)
    rm = rn * (1 - E2) / (1 - E2 * s * s)
    return (
        math.radians(lon - PIREN[1]) * rn * math.cos(lat0),  # east
        math.radians(lat - PIREN[0]) * rm,  # north
    )


def ts(s):
    return datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp()


def load(path):
    return [json.loads(line) for line in open(path) if line.strip()]


def dataset(root, scenario):
    """Ownship and true target positions by time, from the Autoferry files."""
    base = Path(root) / f"scenario{scenario}" / f"scenario{scenario}"
    det = json.load(open(f"{base}_detections.json"))
    gt = json.load(open(f"{base}_groundTruth.json"))
    own = sorted({(d["time"], d["ownshipPosition"][1], d["ownshipPosition"][0]) for d in det})
    truth = {}
    for entry in gt:
        for t in entry if isinstance(entry, list) else [entry]:
            truth.setdefault(t["targetID"], []).append((t["time"], t["position"][1], t["position"][0]))
    return own, {k: sorted(v) for k, v in truth.items()}


def at(series, t):
    """The last sample of (time, x, y) at or before t."""
    best = None
    for s in series:
        if s[0] > t:
            break
        best = s
    return best


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("trace")
    ap.add_argument("fixture")
    ap.add_argument("out")
    ap.add_argument("--dataset")
    args = ap.parse_args()

    name = Path(args.fixture).stem
    scenario = int(name.split("-")[0].removeprefix("scenario"))
    mode = "raw detections" if name.endswith("detections") else "sensor tracks"
    rows = load(args.fixture)
    truth_of = {f"{r['obs']['source_id']}/{r['obs']['source_track_key']}@{r['obs']['observed_at']}": r["truth"] for r in rows}
    names = {r["obs"]["source_track_key"]: r["obs"].get("name") for r in rows if r["feed"] == "track"}

    events = load(args.trace)
    for e in events:
        e["ts"] = ts(e["t"])
        if "lat" in e:
            e["x"], e["y"] = local(e["lat"], e["lon"])
    events.sort(key=lambda e: e["ts"])
    t0, t1 = events[0]["ts"], events[-1]["ts"]

    own, truth = dataset(args.dataset, scenario) if args.dataset else ([], {})
    # Frame on the targets and the ferry (radar clutter reaches far beyond them).
    framed = [(e["x"], e["y"]) for e in events if e.get("source") == "track"]
    framed += [(o[1], o[2]) for o in own] + [(s[1], s[2]) for v in truth.values() for s in v]
    xs, ys = [p[0] for p in framed], [p[1] for p in framed]
    lo, hi = min, max
    cx, cy = (lo(xs) + hi(xs)) / 2, (lo(ys) + hi(ys)) / 2
    half = max(hi(xs) - lo(xs), (hi(ys) - lo(ys)) * 16 / 9) / 2 * 1.25
    xlim, ylim = (cx - half, cx + half), (cy - half * 9 / 16, cy + half * 9 / 16)

    fig = plt.figure(figsize=(12.8, 7.2), dpi=100, facecolor=BG)
    ax = fig.add_axes([0.0, 0.0, 1.0, 1.0])
    writer = imageio_ffmpeg.write_frames(args.out, (1280, 720), fps=FPS, quality=7, macro_block_size=8)
    writer.send(None)

    uid_of = {}  # source/key -> system track uid (as the engine has it now)
    last_seen = {}  # source/key -> time of its newest report
    colour_of = {}  # uid -> colour, for tracks that are not a target's
    history = []  # events applied so far
    banners = []  # (time, text)
    merges = assoc = unassoc = 0
    i = 0
    t = t0
    while t <= t1 + FADE_S:
        while i < len(events) and events[i]["ts"] <= t:
            e = events[i]
            if e["kind"] == "merge":
                moved = [k for k, u in uid_of.items() if u == e["from"]]
                for k in moved:
                    uid_of[k] = e["into"]
                merges += 1
                already = [k for k, u in uid_of.items() if u == e["into"] and k not in moved]

                def show(keys):
                    # Only keys still reporting: a dropped fragment's name is noise.
                    live = [k for k in keys if last_seen.get(k, -1e9) >= e["ts"] - 6] or keys
                    return " + ".join(
                        f"{names.get(k.split('/')[1])} track feed" if k.startswith("track/") else k.replace("/", " ")
                        for k in sorted(live, key=lambda k: k.startswith("track/"))
                    )

                a, b = show(moved), show(already)
                if any(k.startswith("track/") for k in moved):
                    a, b = b, a
                banners.append((e["ts"], f"paired  {a}  ⟷  {b}", e["into"]))
            else:
                key = f"{e['source']}/{e['key']}"
                if e["kind"] == "report":
                    uid_of[key] = e["uid"]
                    last_seen[key] = e["ts"]
                elif e["uid"]:
                    assoc += 1
                else:
                    unassoc += 1
                e["truth"] = truth_of.get(f"{key}@{e['t']}")
                history.append(e)
            i += 1

        target_uid = {k: uid_of.get(f"track/target-{k}") for k in (1, 2)}

        def colour(uid):
            for k, u in target_uid.items():
                if uid and uid == u:
                    return TARGET_COLOURS[k]
            if not uid:
                return MUTED
            if uid not in colour_of:
                colour_of[uid] = OTHER[len(colour_of) % len(OTHER)]
            return colour_of[uid]

        ax.cla()
        ax.set_facecolor(BG)
        ax.set_xlim(*xlim)
        ax.set_ylim(*ylim)
        ax.set_aspect("equal")
        ax.set_xticks([])
        ax.set_yticks([])
        for sp in ax.spines.values():
            sp.set_visible(False)
        # 50 m grid.
        for gx in range(int(xlim[0] // 50) * 50, int(xlim[1]) + 50, 50):
            ax.axvline(gx, color=GRID, lw=0.6, zorder=0)
        for gy in range(int(ylim[0] // 50) * 50, int(ylim[1]) + 50, 50):
            ax.axhline(gy, color=GRID, lw=0.6, zorder=0)

        # True positions (dataset ground truth) as faint rings with the path so far.
        tt = t
        for k, series in truth.items():
            past = [s for s in series if s[0] <= tt]
            if not past:
                continue
            ax.plot([s[1] for s in past], [s[2] for s in past], color=TARGET_COLOURS[k], lw=0.8, alpha=0.25, ls="--", zorder=1)
            ax.scatter([past[-1][1]], [past[-1][2]], s=420, facecolors="none", edgecolors=TARGET_COLOURS[k], alpha=0.35, lw=1, zorder=1)
        o = at(own, tt)
        if o:
            ax.scatter([o[1]], [o[2]], marker="P", s=120, color=FG, zorder=6)
            ax.text(o[1] + 8, o[2] - 14, "milliAmpere (sensors)", color=FG, fontsize=8, zorder=6)

        # Reports: trails per source track, newest marker labelled.
        latest = {}
        for e in history:
            age = t - e["ts"]
            if e["kind"] == "report" and age <= TRAIL_S:
                latest.setdefault(f"{e['source']}/{e['key']}", []).append(e)
        for key, es in latest.items():
            src = key.split("/")[0]
            c = colour(uid_of.get(key))
            ax.plot([e["x"] for e in es], [e["y"] for e in es], color=c, lw=1.2 if src == "track" else 0.9, alpha=0.55, zorder=2)
            last = es[-1]
            if t - last["ts"] > 4:
                continue
            size = {"track": 90, "lidar": 38, "radar": 52}.get(src, 30)
            if src == "track":
                ax.scatter([last["x"]], [last["y"]], marker="s", s=size, facecolors="none", edgecolors=c, lw=2, zorder=5)
                label = names.get(key.split("/")[1]) or key
            else:
                ax.scatter([last["x"]], [last["y"]], marker=MARKERS[src], s=size, color=c, edgecolors=BG, lw=0.5, zorder=4)
                label = key.split("/")[1]
            dx, dy = {"track": (7, 7), "lidar": (5, -12), "radar": (-22, 5)}.get(src, (5, 5))
            ax.text(last["x"] + dx, last["y"] + dy, label, color=c, fontsize=8 if src != "track" else 9, weight="bold" if src == "track" else None, zorder=6)

        # Detections: fade out; associated ones get a line to their track's newest report.
        track_pos = {}
        for key, es in latest.items():
            track_pos.setdefault(uid_of.get(key), (es[-1]["x"], es[-1]["y"]))
            if key.startswith("track/"):
                track_pos[uid_of.get(key)] = (es[-1]["x"], es[-1]["y"])
        for e in history:
            if e["kind"] != "detection":
                continue
            age = t - e["ts"]
            if age > FADE_S:
                continue
            alpha = max(0.15, 1 - age / FADE_S)
            if e["uid"]:
                c = colour(e["uid"])
                ax.scatter([e["x"]], [e["y"]], s=26 if e["source"] == "radar-det" else 14, marker="^" if e["source"] == "radar-det" else "o", color=c, alpha=alpha, zorder=3)
                p = track_pos.get(e["uid"])
                if p and age < 0.8:
                    ax.plot([e["x"], p[0]], [e["y"], p[1]], color=c, lw=0.8, alpha=alpha * 0.8, zorder=3)
            else:
                ax.scatter([e["x"]], [e["y"]], s=22, marker="x", color=MUTED, alpha=alpha, lw=1, zorder=3)

        # Header, counters and legend.
        ax.text(0.012, 0.975, f"Autoferry scenario {scenario} · {mode}", transform=ax.transAxes, color=FG, fontsize=15, weight="bold", va="top")
        stats = f"t = {t - t0:5.1f} s   ×5 speed"
        if mode == "sensor tracks":
            stats += f"   ·   kinematic pairings: {merges}"
        else:
            stats += f"   ·   detections associated: {assoc}   unassociated: {unassoc}"
        ax.text(0.012, 0.925, stats, transform=ax.transAxes, color=MUTED, fontsize=10, va="top", family="monospace")
        legend = [
            ("s", "track feed (ground truth, AIS-like)", None),
            ("o", "lidar", None),
            ("^", "radar", None),
        ]
        if mode == "raw detections":
            legend.append(("x", "detection, no track in gate", None))
        legend.append(("P", "ferry carrying the sensors", None))
        y = 0.075 + 0.035 * (len(legend) - 1)
        for m, text, _ in legend:
            ax.scatter([0.02], [y], transform=ax.transAxes, marker=m, s=40, facecolors="none" if m == "s" else FG, edgecolors=FG, color=FG)
            ax.text(0.035, y, text, transform=ax.transAxes, color=FG, fontsize=9, va="center")
            y -= 0.035
        ax.text(0.99, 0.03, "colour = the system track a report is on · faint ring = true position · grid 50 m", transform=ax.transAxes, color=MUTED, fontsize=9, ha="right")
        by = 0.975
        for bt, text, uid in banners:
            if 0 <= t - bt <= 4:
                ax.text(0.99, by, text, transform=ax.transAxes, color=colour(uid), fontsize=11, ha="right", va="top", weight="bold",
                        bbox={"facecolor": BG, "edgecolor": colour(uid), "boxstyle": "round,pad=0.35", "alpha": 0.9})
                by -= 0.055

        fig.canvas.draw()
        buf = fig.canvas.buffer_rgba()
        import numpy as np

        writer.send(np.asarray(buf)[:, :, :3].tobytes())
        t += STEP
    writer.close()
    print(f"{args.out}: {t1 - t0:.0f} s of scenario")


if __name__ == "__main__":
    main()

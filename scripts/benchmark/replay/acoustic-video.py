#!/usr/bin/env python3
"""Render an acoustic-array scenario (`scenarios/acoustic.py`) run through
`opentrack bench` as a video: fixed arrays, their bearing (+ often range)
reports, the drones they report on, and the system tracks the correlator
makes of them, coloured by track (as `esm-video.py` does for esm-crossfix).

A side panel counts, at every sample, how many confirmed tracks hold reports
from two or more arrays (fused) against how many are still a single array's
fragment of a drone (`docs/non-point-contacts.md`, "Acoustic arrays").

Usage: acoustic-video.py <scenario_dir> <results_dir> <out.mp4>
    <scenario_dir>: scripts/benchmark build output (scenario.json, truth.jsonl,
        <array>.frames.jsonl)
    <results_dir>: `opentrack bench ... --out <results_dir>` output
        (trace.jsonl, tracks.jsonl, run.json)
Needs matplotlib, numpy and imageio-ffmpeg.
"""

import argparse
import json
import math
import sys
from collections import defaultdict
from datetime import datetime, timezone
from pathlib import Path

import imageio_ffmpeg
import matplotlib
import numpy as np

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402
from matplotlib.patches import Circle  # noqa: E402

FPS = 12
BG, FG, MUTED, GRID = "#0f1419", "#e6e8eb", "#6b7580", "#1f2730"
FUSED, FRAG, FALSEC = "#6fd08c", "#f5a524", "#ff5d5d"
PALETTE = ["#f5a524", "#3fb8f0", "#b07cf2", "#6fd08c", "#f06b8a", "#c9c26b", "#8a9bf5", "#e0895c", "#5cc9c0", "#d68ad6",
           "#ffcf5c", "#7fd4ff", "#cfa3ff", "#9be8b0", "#ff9fb5", "#e6df8a", "#b3beff", "#ffb48a", "#8ae6de", "#f0b3f0"]
R = 6371000.0


def jsonl(path):
    return [json.loads(line) for line in open(path) if line.strip()]


def ts(s):
    return datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp()


class Local:
    def __init__(self, lat0, lon0):
        self.lat0, self.lon0 = lat0, lon0
        self.k = math.cos(math.radians(lat0))

    def xy(self, lat, lon):
        return (math.radians(lon - self.lon0) * R * self.k, math.radians(lat - self.lat0) * R)

    def ll(self, e, n):
        return (self.lat0 + math.degrees(n / R), self.lon0 + math.degrees(e / (R * self.k)))


class Colours:
    def __init__(self):
        self.of = {}

    def __call__(self, uid):
        if uid is None:
            return MUTED
        if uid not in self.of:
            self.of[uid] = PALETTE[len(self.of) % len(PALETTE)]
        return self.of[uid]


def style(ax, xlim, ylim, grid=100):
    ax.cla()
    ax.set_facecolor(BG)
    ax.set_xlim(*xlim)
    ax.set_ylim(*ylim)
    ax.set_aspect("equal")
    ax.set_xticks([])
    ax.set_yticks([])
    for sp in ax.spines.values():
        sp.set_visible(False)
    for g in np.arange(math.floor(xlim[0] / grid) * grid, xlim[1] + grid, grid):
        ax.axvline(g, color=GRID, lw=0.6, zorder=0)
    for g in np.arange(math.floor(ylim[0] / grid) * grid, ylim[1] + grid, grid):
        ax.axhline(g, color=GRID, lw=0.6, zorder=0)


def legend(ax, items, x=0.02, y0=0.05, dy=0.04, scale=1.0):
    y = y0 + dy * (len(items) - 1)
    for kind, text, c in items:
        if kind == "line":
            ax.plot([x - 0.008, x + 0.012], [y, y], transform=ax.transAxes, color=c, lw=1.4)
        elif kind == "dash":
            ax.plot([x - 0.008, x + 0.012], [y, y], transform=ax.transAxes, color=c, lw=1.2, ls=(0, (3, 2)))
        elif kind == "ring":
            ax.add_patch(Circle((x + 0.002, y), 0.01, transform=ax.transAxes, fill=False, ec=c, alpha=0.6))
        else:
            ax.scatter([x + 0.002], [y], transform=ax.transAxes, marker=kind, s=40 * scale, color=c)
        ax.text(x + 0.022, y, text, transform=ax.transAxes, color=FG, fontsize=9 * scale, va="center")
        y -= dy


class Video:
    def __init__(self, out):
        self.fig = plt.figure(figsize=(12.8, 7.2), dpi=100, facecolor=BG)
        self.w = imageio_ffmpeg.write_frames(out, (1280, 720), fps=FPS, quality=6, macro_block_size=8,
                                              output_params=["-movflags", "+faststart"])
        self.w.send(None)
        self.out = out

    def send(self):
        self.fig.canvas.draw()
        self.w.send(np.asarray(self.fig.canvas.buffer_rgba())[:, :, :3].tobytes())

    def close(self):
        self.w.close()
        print(self.out)


def load_scenario(scenario_dir):
    scenario = json.loads((scenario_dir / "scenario.json").read_text())
    notes = scenario.get("notes") or {}
    arrays = {a["id"]: a for a in notes.get("arrays", [])}
    lat0 = sum(a["site"][0] for a in arrays.values()) / len(arrays)
    lon0 = sum(a["site"][1] for a in arrays.values()) / len(arrays)
    local = Local(lat0, lon0)
    detections = defaultdict(lambda: defaultdict(list))  # array_id -> t -> [records]
    for aid in arrays:
        for row in jsonl(scenario_dir / f"{aid}.frames.jsonl"):
            for rec in row["json"]["detections"]:
                detections[aid][rec["t"]].append(rec)
    truth = defaultdict(dict)  # drone_id -> t -> (lat, lon, alt, scored)
    for p in jsonl(scenario_dir / "truth.jsonl"):
        t = ts(p["t"])
        truth[p["id"]][t] = (p["lat"], p["lon"], p.get("alt"), p.get("score", True))
    return scenario, arrays, local, detections, truth


def load_run(results_dir):
    trace = jsonl(results_dir / "trace.jsonl")
    for e in trace:
        e["ts"] = ts(e["t"]) if "t" in e else None
    tracks_by_t = defaultdict(list)
    for r in jsonl(results_dir / "tracks.jsonl"):
        tracks_by_t[round(ts(r["t"]), 3)].append(r)
    return trace, tracks_by_t


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("scenario_dir", type=Path)
    ap.add_argument("results_dir", type=Path)
    ap.add_argument("out")
    args = ap.parse_args()

    scenario, arrays, local, detections, truth = load_scenario(args.scenario_dir)
    trace, tracks_by_t = load_run(args.results_dir)
    sample_times = sorted(tracks_by_t)
    if not sample_times:
        sys.exit("no samples in tracks.jsonl")

    # Frame the whole field: every array's range circle and every drone's path.
    xs, ys = [], []
    for a in arrays.values():
        x, y = local.xy(a["site"][0], a["site"][1])
        xs += [x - a["range_m"], x + a["range_m"]]
        ys += [y - a["range_m"], y + a["range_m"]]
    cx, cy = (min(xs) + max(xs)) / 2, (min(ys) + max(ys)) / 2
    half = max(max(xs) - min(xs), (max(ys) - min(ys)) * 16 / 9) / 2 * 1.08
    xlim, ylim = (cx - half, cx + half), (cy - half * 9 / 16, cy + half * 9 / 16)
    grid = 100.0 if half < 800 else 200.0

    colour = Colours()
    uid_of = {}  # "source/key" -> system track uid
    merges_done = 0
    i = 0
    trail = defaultdict(list)

    v = Video(args.out)
    ax = v.fig.add_axes([0.0, 0.0, 900 / 1280, 1.0])
    side = v.fig.add_axes([900 / 1280, 0.0, 380 / 1280, 1.0])

    for T in sample_times:
        while i < len(trace) and trace[i]["ts"] is not None and trace[i]["ts"] <= T:
            e = trace[i]
            if e["kind"] == "merge":
                for k, u in list(uid_of.items()):
                    if u == e["from"]:
                        uid_of[k] = e["into"]
                merges_done += 1
            elif e["kind"] == "report":
                uid_of[f"{e['source']}/{e['key']}"] = e["uid"]
            i += 1

        tracks_now = tracks_by_t.get(T, [])
        confirmed = [t for t in tracks_now if t["state"] == "confirmed"]
        fused = frags = 0
        for t in confirmed:
            n_arrays = len({c.split("/", 1)[0] for c in (t.get("contributors") or [])})
            if n_arrays >= 2:
                fused += 1
            else:
                frags += 1

        style(ax, xlim, ylim, grid=grid)
        off = half * 0.018
        # Arrays: fixed sensors with a dashed range circle.
        for aid, a in arrays.items():
            x, y = local.xy(a["site"][0], a["site"][1])
            ax.add_patch(Circle((x, y), a["range_m"], fill=False, ec=MUTED, alpha=0.35, lw=1, ls=(0, (2, 3)), zorder=1))
            ax.scatter([x], [y], marker="^", s=90, color=FG, zorder=7)
            ax.text(x + off, y - 2.5 * off, aid, color=FG, fontsize=8, zorder=7)
        # Bearing (+ range) lines this second, coloured by track.
        for aid, by_t in detections.items():
            a = arrays[aid]
            ax_, ay_ = local.xy(a["site"][0], a["site"][1])
            for rec in by_t.get(T, []):
                uid = uid_of.get(f"{aid}/{rec['id']}")
                false_alarm = rec["id"].startswith("x")
                c = FALSEC if false_alarm else colour(uid)
                reach = rec["range"] if rec["range"] is not None else a["range_m"]
                b = math.radians(rec["bearing"])
                x1, y1 = ax_ + reach * math.sin(b), ay_ + reach * math.cos(b)
                ls = "-" if rec["range"] is not None else (0, (4, 3))
                alpha = 0.85 if uid else 0.4
                ax.plot([ax_, x1], [ay_, y1], color=c, alpha=alpha, lw=1.1 if rec["range"] is not None else 0.8, ls=ls, zorder=2)
        # Truth drones: faint ring, short trail, dimmed once outside every
        # array's range (don't-care for scoring).
        for did, series in truth.items():
            p = series.get(T)
            if p is None:
                continue
            lat, lon, alt, scored = p
            x, y = local.xy(lat, lon)
            trail[did].append((x, y))
            trail[did] = trail[did][-15:]
            a_ = 0.5 if scored else 0.2
            ax.plot([q[0] for q in trail[did]], [q[1] for q in trail[did]], color=FG, lw=0.8, alpha=a_ * 0.6, ls=":", zorder=1)
            ax.add_patch(Circle((x, y), off * 0.4, fill=False, ec=FG, alpha=a_, lw=1, zorder=1))
            ax.text(x + off, y + off, did, color=MUTED, fontsize=7, alpha=a_, zorder=1)
        # System tracks: filled square if confirmed, hollow if tentative.
        for t in tracks_now:
            x, y = local.xy(t["lat"], t["lon"])
            c = colour(t["uid"])
            if t["state"] == "confirmed":
                ax.scatter([x], [y], marker="s", s=70, color=c, edgecolors=BG, lw=0.8, zorder=6)
            else:
                ax.scatter([x], [y], marker="s", s=55, facecolors="none", edgecolors=c, lw=1.2, alpha=0.7, zorder=6)
            n_arrays = len({cc.split("/", 1)[0] for cc in (t.get("contributors") or [])})
            if n_arrays >= 2:
                ax.text(x + off, y - 2 * off, f"×{n_arrays}", color=c, fontsize=7, weight="bold", zorder=6)
        ax.text(0.99, 0.012, f"grid {grid:.0f} m", transform=ax.transAxes, color=MUTED, fontsize=8, ha="right")

        side.cla()
        side.set_facecolor(BG)
        side.axis("off")
        side.set_xlim(0, 1)
        side.set_ylim(0, 1)
        side.text(0.05, 0.965, scenario["name"], color=FG, fontsize=16, weight="bold", va="top")
        side.text(0.05, 0.915, f"t = {T:5.0f} s", color=MUTED, fontsize=11, family="monospace", va="top")
        side.text(0.05, 0.87, scenario.get("description", ""), color=FG, fontsize=8.3, va="top", wrap=True, linespacing=1.4)
        stats = [
            ("arrays", f"{len(arrays)}"),
            ("drones", f"{len(truth)}"),
            ("confirmed tracks", f"{len(confirmed)}"),
            ("fused (2+ arrays)", f"{fused}"),
            ("single-array fragments", f"{frags}"),
            ("merges so far", f"{merges_done}"),
        ]
        y = 0.60
        for name, value in stats:
            side.text(0.05, y, name, color=MUTED, fontsize=10, va="top")
            side.text(0.63, y, value, color=FRAG if name == "single-array fragments" and frags else
                      (FUSED if name == "fused (2+ arrays)" and fused else FG),
                      fontsize=10, family="monospace", va="top")
            y -= 0.045
        legend(side, [
            ("line", "bearing + range, on a track (its colour)", PALETTE[1]),
            ("dash", "bearing only (no range this report)", MUTED),
            ("line", "false alarm (no drone)", FALSEC),
            ("s", "confirmed track", FG),
            ("^", "acoustic array", FG),
            ("ring", "true drone position", FG),
        ], x=0.06, y0=0.08, dy=0.042)
        v.send()
    v.close()


if __name__ == "__main__":
    main()

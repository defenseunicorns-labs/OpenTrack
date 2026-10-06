#!/usr/bin/env python3
"""OpenTrack tracker and correlation benchmark.

    bench.py list                         scenarios, and which are built
    bench.py build [NAME... | all]        fetch the data and build scenarios
    bench.py run [NAME... | all]          run them through OpenTrack and score
        [--label L] [--correlation settings.json] [--set SOURCE.path=VALUE]...
        [--profile SOURCE=NAME]... [--plugin FILE_OR_ADDRESS]... [--redis URL] [--no-build]
    bench.py score RUN                    score a run again
    bench.py compare RUN_A RUN_B          the measures side by side

A run is `opentrack bench` over each scenario: the real pipelines (codecs,
mappings, tracker stages) and the real engine, on the scenario's clock, as
fast as they go, publishing nothing. Scores go to
$OT_BENCH_DATA/results/<run>/ (default ~/data/benchmark): score.json per
scenario, and summary.md / summary.json for the run.

Data never goes in the repository: downloads, built scenarios and results
all live under $OT_BENCH_DATA. See README.md for the scenarios and measures.
"""

import argparse
import json
import socket
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

from common import REPO, results_dir, scenarios_dir  # noqa: E402


def all_scenarios():
    from scenarios import SCENARIOS

    return SCENARIOS


def pick(names, available):
    if not names or names == ["all"]:
        return list(available)
    out = []
    for n in names:
        matched = [a for a in available if a == n or (n.endswith("*") and a.startswith(n[:-1]))]
        if not matched:
            sys.exit(f"unknown scenario {n!r}; see `bench.py list`")
        out += [m for m in matched if m not in out]
    return out


def cmd_list(_):
    for name in all_scenarios():
        built = (scenarios_dir() / name / "scenario.json").exists()
        print(f"{'built  ' if built else '       '} {name}")


def cmd_build(args):
    scenarios = all_scenarios()
    failed = []
    for name in pick(args.names, scenarios):
        try:
            scenarios[name]()
        except SystemExit as e:
            print(f"{name}: skipped: {e}")
            failed.append(name)
    if failed:
        print(f"not built: {', '.join(failed)}")


def git_describe():
    def run(*a):
        return subprocess.run(["git", "-C", str(REPO), *a], capture_output=True, text=True).stdout.strip()

    return {"commit": run("rev-parse", "--short", "HEAD"), "dirty": bool(run("status", "--porcelain", "--", "crates"))}


class Redis:
    """A throwaway Redis in Docker, unless one is given."""

    def __init__(self, url):
        self.url, self.container = url, None

    def __enter__(self):
        if self.url:
            return self.url
        s = socket.socket()
        s.bind(("127.0.0.1", 0))
        port = s.getsockname()[1]
        s.close()
        self.container = f"ot-bench-redis-{port}"
        subprocess.run(["docker", "run", "-d", "--rm", "--name", self.container, "-p", f"127.0.0.1:{port}:6379",
                        "redis:7-alpine", "redis-server", "--save", "", "--appendonly", "no"],
                       check=True, capture_output=True)
        for _ in range(50):
            try:
                socket.create_connection(("127.0.0.1", port), timeout=0.2).close()
                break
            except OSError:
                time.sleep(0.1)
        return f"redis://127.0.0.1:{port}"

    def __exit__(self, *exc):
        if self.container:
            subprocess.run(["docker", "stop", self.container], capture_output=True)


def binary(args):
    exe = REPO / "target/release/opentrack"
    if not args.no_build:
        subprocess.run(["cargo", "build", "--release", "-q", "-p", "ot-server"], cwd=REPO, check=True)
    if not exe.exists():
        sys.exit(f"{exe} missing: build it (cargo build --release -p ot-server)")
    return exe


def apply_sets(scenario_dir, sets, dest):
    """A copy of the scenario (frames and truth linked) with source spec
    fields changed: `radar.pipeline.tracker.confirm_hits=5`, where the first
    part is a source id or `*` (every source that has the field's parent).
    Values are JSON, or else strings. None when no change applies."""
    doc = json.loads((scenario_dir / "scenario.json").read_text())
    changed = False
    for item in sets:
        path, _, raw = item.partition("=")
        source, *keys = path.split(".")
        try:
            value = json.loads(raw)
        except json.JSONDecodeError:
            value = raw
        for s in doc["sources"]:
            spec = s["spec"]
            if source not in ("*", spec["id"]):
                continue
            node = spec
            for k in keys[:-1]:
                if not isinstance(node.get(k), dict):
                    if source != "*":
                        sys.exit(f"--set {item}: {spec['id']} has no {k}")
                    node = None
                    break
                node = node[k]
            if node is not None:
                node[keys[-1]] = value
                changed = True
    if not changed:
        return None
    dest.mkdir(parents=True, exist_ok=True)
    for f in scenario_dir.iterdir():
        if f.name != "scenario.json" and not (dest / f.name).exists():
            (dest / f.name).symlink_to(f)
    (dest / "scenario.json").write_text(json.dumps(doc, indent=2) + "\n")
    return dest


def profile_sets(profiles):
    """`SOURCE=NAME` → a --set that gives the source that tracker profile
    (profiles/trackers/NAME.json; SOURCE may be `*`)."""
    out = []
    for item in profiles or []:
        source, _, name = item.partition("=")
        path = REPO / "profiles/trackers" / f"{name}.json"
        if not name or not path.exists():
            sys.exit(f"--profile {item}: no profile {path}")
        tracker = dict(json.loads(path.read_text())["tracker"], profile=name)
        out.append(f"{source}.pipeline.tracker={json.dumps(tracker)}")
    return out


def cmd_run(args):
    import scoring

    args.set = (args.set or []) + profile_sets(args.profile)
    names = [n for n in pick(args.names, all_scenarios()) if (scenarios_dir() / n / "scenario.json").exists()]
    if not names:
        sys.exit("no built scenarios: run `bench.py build` first")
    exe = binary(args)
    stamp = datetime.now(timezone.utc).strftime("%Y%m%d-%H%M%S")
    run_id = stamp + (f"-{args.label}" if args.label else "")
    out = results_dir() / run_id
    out.mkdir(parents=True)
    meta = {"run": run_id, "git": git_describe(), "correlation": args.correlation, "set": args.set or [],
            "plugins": args.plugin or [],
            "scenarios": names}
    (out / "run.json").write_text(json.dumps(meta, indent=2) + "\n")
    scores = []
    with Redis(args.redis) as url:
        for name in names:
            dest = out / name
            scenario = scenarios_dir() / name
            if args.set:
                scenario = apply_sets(scenario, args.set, out / "scenarios" / name) or scenario
            cmd = [str(exe), "--redis", url, "bench", str(scenario), "--out", str(dest)]
            if args.correlation:
                cmd += ["--correlation", str(Path(args.correlation).resolve())]
            for plugin in args.plugin or []:
                cmd += ["--plugin", str(Path(plugin).resolve()) if Path(plugin).exists() else plugin]
            r = subprocess.run(cmd, env={"RUST_LOG": "warn", "PATH": "/usr/bin:/bin"}, capture_output=True, text=True)
            if r.returncode != 0:
                print(f"{name}: failed\n{r.stderr[-2000:]}")
                continue
            print(r.stderr.strip().splitlines()[-1] if r.stderr.strip() else name)
            notes = json.loads((scenario / "scenario.json").read_text()).get("notes") or {}
            if notes.get("load"):
                run = json.loads((dest / "run.json").read_text())
                t = run["timing_secs"]
                print(f"  {run['observations'] / run['wall_secs']:.0f} observations/s overall; engine "
                      f"{run['observations'] / max(t['engine'], 1e-9):.0f}/s ({t['engine']:.1f} s), pipelines "
                      f"{t['pipelines']:.1f} s")
                continue
            scores.append(scoring.score(scenario, dest))
            (dest / "scenario").write_text(str(scenario) + "\n")
    summarise(out, meta, scores)


def cmd_score(args):
    import scoring

    out = results_dir() / args.run
    meta = json.loads((out / "run.json").read_text())
    def scenario_of(n):
        pointer = out / n / "scenario"
        return Path(pointer.read_text().strip()) if pointer.exists() else scenarios_dir() / n

    scores = [scoring.score(scenario_of(n), out / n) for n in meta["scenarios"] if (out / n / "run.json").exists()]
    summarise(out, meta, scores)


def fmt(v, digits=2, pct=False):
    if v is None:
        return "–"
    if pct:
        return f"{100 * v:.0f}%"
    return f"{v:.{digits}f}" if isinstance(v, float) else str(v)


COLUMNS = [
    ("tracks", lambda s: f"{s['system']['tracks']} ({fmt(s['system']['live_tracks_mean'], 1)} live, "
                         f"{fmt(s['system']['live_truths_mean'], 1)} true)"),
    ("GOSPA", lambda s: fmt(s["system"]["gospa"], 1)),
    ("loc / miss / false", lambda s: " / ".join(fmt(s["system"][k], 0) for k in ("gospa_localisation", "gospa_missed", "gospa_false"))),
    ("complete", lambda s: fmt(s["system"]["completeness"], pct=True)),
    ("spurious", lambda s: fmt(s["system"]["spuriousness"], pct=True) if s["truth_complete"] else "n/a"),
    ("pos RMS m", lambda s: fmt(s["system"]["position_rms_m"], 1)),
    ("ID chg/h", lambda s: fmt(s["system"]["id_changes_per_truth_hour"], 1)),
    ("frag", lambda s: fmt(s["system"]["fragmentation"], 2)),
    ("longest", lambda s: fmt(s["system"]["longest_segment"], pct=True)),
    ("IDF1", lambda s: fmt(s["system"].get("idf1"), pct=True)),
    ("pairing P / R", lambda s: " / ".join(fmt(s["correlation"][k], pct=True) for k in ("pairing_precision", "pairing_recall")) if "correlation" in s else "–"),
    ("× real time", lambda s: fmt(s["speed_x_real_time"], 0)),
]


def table(scores):
    lines = ["| scenario | " + " | ".join(c for c, _ in COLUMNS) + " |", "|---" * (len(COLUMNS) + 1) + "|"]
    for s in scores:
        lines.append(f"| {s['scenario']} | " + " | ".join(f(s) for _, f in COLUMNS) + " |")
    return "\n".join(lines)


def tracker_table(scores):
    lines = ["| scenario | tracker | tracks | GOSPA | complete | spurious | pos RMS m | ID chg/h | frag |", "|---" * 9 + "|"]
    for s in scores:
        for src, t in s["trackers"].items():
            lines.append(
                f"| {s['scenario']} | {src} | {t['tracks']} ({fmt(t['live_tracks_mean'], 1)} live) | {fmt(t['gospa'], 1)} | {fmt(t['completeness'], pct=True)} | "
                f"{fmt(t['spuriousness'], pct=True) if s['truth_complete'] else 'n/a'} | {fmt(t['position_rms_m'], 1)} | "
                f"{fmt(t['id_changes_per_truth_hour'], 1)} | {fmt(t['fragmentation'], 2)} |")
    return "\n".join(lines)


# The roadmap's harbour gate: wrong pairings over the harbour fusion
# scenarios (Autoferry in Trondheim harbour, and the Solent), pooled.
HARBOUR = ("autoferry-", "solent-")


def harbour(scores, key="correlation"):
    """Pooled pairing over the harbour fusion scenarios run, or None."""
    right = wrong = missed = 0
    for s in scores:
        c = s.get(key)
        if not c or not s["scenario"].endswith("-fusion") or not s["scenario"].startswith(HARBOUR):
            continue
        right, wrong, missed = right + c["pairs_right"], wrong + c["pairs_wrong"], missed + c["pairings_missed"]
    if not right + wrong:
        return None
    return {"wrong": round(wrong / (right + wrong), 4), "recall": round(right / (right + missed), 4),
            "pairs_right": right, "pairs_wrong": wrong}


def summarise(out, meta, scores):
    gate = harbour(scores)
    strict = harbour(scores, "correlation_per_sample")
    if gate and strict:
        gate["per_sample_wrong"] = strict["wrong"]
    (out / "summary.json").write_text(json.dumps({"meta": meta, "scores": scores, "harbour": gate}, indent=2) + "\n")
    git = meta["git"]
    md = [
        f"# Benchmark {meta['run']}",
        "",
        f"OpenTrack {git['commit']}{' (uncommitted changes)' if git['dirty'] else ''}"
        + (f", correlation settings {meta['correlation']}" if meta.get("correlation") else "")
        + (f", with {'; '.join(meta['set'])}" if meta.get("set") else "")
        + (f", plugins {', '.join(Path(p).name for p in meta['plugins'])}" if meta.get("plugins") else ""),
        "",
        "## System tracks (after correlation)",
        "",
        table(scores),
        "",
        *([f"Harbour gate (Autoferry and Solent fusion, pooled): {gate['wrong'] * 100:.2f}% wrong pairings "
           f"(target under 2%), recall {gate['recall'] * 100:.1f}%"
           + (f"; judged at each moment, {gate['per_sample_wrong'] * 100:.2f}%." if 'per_sample_wrong' in gate else "."),
           ""] if gate else []),
        "## Tracker stages (each sensor's own tracks)",
        "",
        tracker_table(scores),
        "",
        "tracks: distinct confirmed tracks over the run (mean live at a time; truths in view). GOSPA: mean per sample, p = 1, alpha = 2 (lower is better), split into localisation, missed and "
        "false. complete: share of truth samples with a track. spurious: share of tracks near no truth. "
        "ID chg/h: track number changes per truth hour. frag: distinct tracks per truth. longest: share of "
        "a truth's time on its longest-held track. IDF1: identity F1, tracks matched one to one to truths over the run. pairing P / R: correlation precision and recall.",
    ]
    (out / "summary.md").write_text("\n".join(md) + "\n")
    print()
    print(table(scores))
    if gate:
        print(f"\nHarbour gate: {gate['wrong'] * 100:.2f}% wrong pairings (target under 2%), recall {gate['recall'] * 100:.1f}%"
              + (f"; at each moment {gate['per_sample_wrong'] * 100:.2f}%" if "per_sample_wrong" in gate else ""))
    print(f"\n{out / 'summary.md'}")


def cmd_compare(args):
    a = json.loads((results_dir() / args.a / "summary.json").read_text())
    b = json.loads((results_dir() / args.b / "summary.json").read_text())
    sa = {s["scenario"]: s for s in a["scores"]}
    keys = [("gospa", "GOSPA", False), ("completeness", "complete", True), ("spuriousness", "spurious", False),
            ("position_rms_m", "pos RMS", False), ("id_changes_per_truth_hour", "ID chg/h", False),
            ("fragmentation", "frag", False)]
    print(f"| scenario | {' | '.join(k[1] for k in keys)} | pairing P | pairing R |")
    print("|---" * (len(keys) + 3) + "|")
    for s in b["scores"]:
        old = sa.get(s["scenario"])
        cells = []
        for k, _, higher_better in keys:
            new_v = s["system"].get(k)
            old_v = old["system"].get(k) if old else None
            cells.append(delta(old_v, new_v, higher_better))
        for k in ("pairing_precision", "pairing_recall"):
            new_v = (s.get("correlation") or {}).get(k)
            old_v = ((old or {}).get("correlation") or {}).get(k)
            cells.append(delta(old_v, new_v, True))
        print(f"| {s['scenario']} | " + " | ".join(cells) + " |")


def delta(old, new, higher_better):
    if new is None:
        return "–"
    if old is None:
        return f"{new:.3g}"
    d = new - old
    if abs(d) < 1e-9:
        return f"{new:.3g}"
    better = (d > 0) == higher_better
    return f"{new:.3g} ({'+' if d > 0 else ''}{d:.2g} {'better' if better else 'worse'})"


def main():
    # `live` has its own options (see live_load.py).
    if len(sys.argv) > 1 and sys.argv[1] == "live":
        import live_load
        live_load.main(sys.argv[2:])
        return
    # So does `swarm` (see swarm.py).
    if len(sys.argv) > 1 and sys.argv[1] == "swarm":
        import swarm
        swarm.main(sys.argv[2:])
        return
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("list").set_defaults(fn=cmd_list)
    p = sub.add_parser("build")
    p.add_argument("names", nargs="*")
    p.set_defaults(fn=cmd_build)
    p = sub.add_parser("run")
    p.add_argument("names", nargs="*")
    p.add_argument("--label")
    p.add_argument("--correlation", help="correlation settings JSON to use instead of each scenario's")
    p.add_argument("--set", action="append", metavar="SOURCE.path=VALUE",
                   help="change a source spec field for this run (repeatable), e.g. gmti.pipeline.tracker.confirm_hits=5")
    p.add_argument("--profile", action="append", metavar="SOURCE=NAME",
                   help="give a source a tracker profile (profiles/trackers/NAME.json); SOURCE may be *")
    p.add_argument("--plugin", action="append", metavar="FILE_OR_ADDRESS",
                   help="load a plugin for the run (.wasm or an external plugin's address); repeatable")
    p.add_argument("--redis", help="Redis URL (default: a throwaway Redis in Docker)")
    p.add_argument("--no-build", action="store_true", help="use target/release/opentrack as it is")
    p.set_defaults(fn=cmd_run)
    p = sub.add_parser("score")
    p.add_argument("run")
    p.set_defaults(fn=cmd_score)
    p = sub.add_parser("compare")
    p.add_argument("a")
    p.add_argument("b")
    p.set_defaults(fn=cmd_compare)
    args = ap.parse_args()
    args.fn(args)


if __name__ == "__main__":
    main()


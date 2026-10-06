"""Shared helpers: where the data lives, times, local coordinates, writers."""

import json
import math
import os
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
SPECS = HERE / "specs"


def data_dir() -> Path:
    """Everything the benchmark downloads, builds and writes: never in the repo."""
    return Path(os.environ.get("OT_BENCH_DATA", Path.home() / "data" / "benchmark")).expanduser()


def raw_dir() -> Path:
    return data_dir() / "raw"


def scenarios_dir() -> Path:
    return data_dir() / "scenarios"


def results_dir() -> Path:
    return data_dir() / "results"


def iso(t: float) -> str:
    return datetime.fromtimestamp(t, timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


def ts(s: str) -> float:
    return datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp()


def fetch(url: str, dest: Path) -> Path:
    """Download once; later builds use the copy."""
    if dest.exists() and dest.stat().st_size > 0:
        return dest
    dest.parent.mkdir(parents=True, exist_ok=True)
    print(f"fetching {url}")
    tmp = dest.with_suffix(dest.suffix + ".part")
    with urllib.request.urlopen(url, timeout=120) as r, open(tmp, "wb") as f:
        while chunk := r.read(1 << 20):
            f.write(chunk)
    tmp.rename(dest)
    return dest


def spec(template: str, **changes) -> dict:
    """A source spec template from specs/, with top-level fields changed."""
    s = json.loads((SPECS / f"{template}.json").read_text())
    s.update(changes)
    return s


class Local:
    """A flat east/north frame (metres) around a reference point: plenty for
    the tens to hundreds of kilometres a scenario covers."""

    R = 6378137.0

    def __init__(self, lat0: float, lon0: float):
        self.lat0, self.lon0 = lat0, lon0
        self.k = math.cos(math.radians(lat0))

    def en(self, lat, lon):
        return (
            math.radians(lon - self.lon0) * self.R * self.k,
            math.radians(lat - self.lat0) * self.R,
        )

    def ll(self, e, n):
        return (
            self.lat0 + math.degrees(n / self.R),
            self.lon0 + math.degrees(e / (self.R * self.k)),
        )


def course_speed(ve: float, vn: float):
    speed = math.hypot(ve, vn)
    return (math.degrees(math.atan2(ve, vn)) % 360 if speed > 0.3 else None), round(speed, 2)


class Writer:
    """Writes a scenario directory: scenario.json, frames per source, truth."""

    def __init__(self, name: str):
        self.name = name
        self.dir = scenarios_dir() / name
        self.dir.mkdir(parents=True, exist_ok=True)
        self.frames: dict[str, list] = {}
        self.truth: list = []

    def frame(self, source: str, t: float, *, json_=None, text=None, hex_=None):
        f = {"t": iso(t)}
        if json_ is not None:
            f["json"] = json_
        elif text is not None:
            f["text"] = text
        else:
            f["hex"] = hex_
        self.frames.setdefault(source, []).append((t, f))

    def truth_point(self, t: float, target, lat: float, lon: float, alt=None, score: bool = True):
        """A true position. `score: false` marks a don't-care point (outside
        every sensor's view): neither missed nor a false track there."""
        p = {"t": iso(t), "id": str(target), "lat": round(lat, 7), "lon": round(lon, 7)}
        if alt is not None:
            p["alt"] = round(alt, 1)
        if not score:
            p["score"] = False
        self.truth.append((t, p))

    def finish(self, description: str, sources: list[dict], **extra):
        entries = []
        for s in sources:
            rows = sorted(self.frames.get(s["id"], []), key=lambda r: r[0])
            fname = f"{s['id']}.frames.jsonl"
            with open(self.dir / fname, "w") as f:
                for _, row in rows:
                    f.write(json.dumps(row, separators=(",", ":")) + "\n")
            entries.append({"spec": s, "frames": fname})
            print(f"  {s['id']}: {len(rows)} frames")
        self.truth.sort(key=lambda r: r[0])
        with open(self.dir / "truth.jsonl", "w") as f:
            for _, p in self.truth:
                f.write(json.dumps(p, separators=(",", ":")) + "\n")
        doc = {"name": self.name, "description": description, "sources": entries, "truth": "truth.jsonl"}
        doc.update(extra)
        (self.dir / "scenario.json").write_text(json.dumps(doc, indent=2) + "\n")
        ids = {p["id"] for _, p in self.truth}
        print(f"  truth: {len(self.truth)} points, {len(ids)} objects -> {self.dir}")

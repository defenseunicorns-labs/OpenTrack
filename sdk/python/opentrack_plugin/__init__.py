"""Write OpenTrack plugins in Python.

A plugin provides any of three kinds (the interface is wit/plugin.wit):

- a Codec: a frame's bytes to JSON records a source's mapping reads
- a Tracker: plots in, tracks out, as a source's tracker stage
- a Scorer: evidence whether a report and a system track are the same
  object, for the engine's pairing test

Subclass the kinds you provide and describe them in a Plugin:

    from opentrack_plugin import Plugin, Tracker, main

    class Nearest(Tracker):
        def __init__(self, options): ...
        def push(self, plot, received_at_ms): ...
        def run(self, now_ms, force): return [...]

    PLUGIN = Plugin({"name": "nearest", "version": "1", "description": "..."}, tracker=Nearest)

    if __name__ == "__main__":
        main(PLUGIN)

Then either run it as an external plugin, with whatever it imports (numpy,
Stone Soup, a GPU):

    OT_PLUGIN_SECRET=<secret> python nearest.py serve --address 127.0.0.1:47300

and add `127.0.0.1:47300` with that secret in Settings -> Plugins; or, if it is plain
Python, build it into a WebAssembly component that runs sandboxed inside
OpenTrack:

    python nearest.py build -o nearest.wasm

Plots, tracks and reports are observations in OpenTrack's schema, as dicts
(docs/plugins.md): `position.latitude`, `observed_at` (ISO 8601),
`source_track_key`, `kinematics.course_deg` and so on.
"""

from __future__ import annotations

import json
import sys
from datetime import datetime, timezone
from typing import Any, Callable

__all__ = [
    "Codec",
    "Tracker",
    "Scorer",
    "Plugin",
    "PluginError",
    "main",
    "log",
    "observed_ms",
    "iso",
    "track",
]


class PluginError(Exception):
    """An error OpenTrack reports for the call (a bad option, a bad frame)."""


class Codec:
    """One stream's decoder: state carries across its frames."""

    def __init__(self, options: dict[str, Any]):
        self.options = options

    def decode(self, frame: bytes, received_at_ms: int) -> list[dict[str, Any]]:
        raise NotImplementedError

    def hints(self) -> dict[str, Any] | None:
        """What the stream has shown about the sensor, once known:
        {"revisit_secs": ..., "revisit_source": "measured"}."""
        return None


class Tracker:
    """One source's tracker stage."""

    def __init__(self, options: dict[str, Any]):
        self.options = options

    def push(self, plot: dict[str, Any], received_at_ms: int) -> None:
        raise NotImplementedError

    def run(self, now_ms: int, force: bool) -> list[dict[str, Any]]:
        """Tracks to report now: observations whose `source_track_key` names
        the track. Called after every frame and about once a second;
        `force` at the end of a stream."""
        raise NotImplementedError


class Scorer:
    """Evidence for the engine's pairing test."""

    def __init__(self, options: dict[str, Any]):
        self.options = options

    def score(self, report: dict[str, Any], candidates: list[dict[str, Any]]) -> list[dict[str, Any]]:
        """One {"ln_lr", "pass", "evidence"} per candidate, in order. Each
        candidate is {"view": <the system track>, "kinematic": <OpenTrack's
        own comparison: distance_m, dt_s, sigma_m, d2, dof, ln_lr, pass>}."""
        raise NotImplementedError


KINDS = ("codec", "tracker", "scorer")


class Plugin:
    """A plugin: its manifest and the classes that provide each kind."""

    def __init__(
        self,
        manifest: dict[str, Any],
        codec: type[Codec] | None = None,
        tracker: type[Tracker] | None = None,
        scorer: type[Scorer] | None = None,
    ):
        self.classes = {"codec": codec, "tracker": tracker, "scorer": scorer}
        self.manifest = dict(manifest)
        self.manifest.setdefault("kinds", [k for k in KINDS if self.classes[k]])
        self.manifest.setdefault("options", [])
        for key in ("name", "version"):
            if not self.manifest.get(key):
                raise ValueError(f"the manifest needs a {key}")

    def describe(self) -> dict[str, Any]:
        return self.manifest

    def defaults(self) -> dict[str, Any]:
        out = {}
        for o in self.manifest.get("options", []):
            if o.get("default") is not None:
                out[o["name"]] = o["default"]
        return out

    def open(self, kind: str, options: dict[str, Any] | None):
        cls = self.classes.get(kind)
        if cls is None:
            raise PluginError(f"this plugin is not a {kind}")
        merged = self.defaults()
        merged.update(options or {})
        return cls(merged)


# --- helpers for observations ---


def observed_ms(obs: dict[str, Any]) -> int:
    """An observation's `observed_at`, in milliseconds since the epoch."""
    t = obs["observed_at"]
    return int(datetime.fromisoformat(t.replace("Z", "+00:00")).timestamp() * 1000)


def iso(ms: int) -> str:
    return datetime.fromtimestamp(ms / 1000, timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


def track(
    plot: dict[str, Any],
    key: str,
    lat: float,
    lon: float,
    *,
    course: float | None = None,
    speed: float | None = None,
    at_ms: int | None = None,
    dropped: bool = False,
) -> dict[str, Any]:
    """A track report made from the plot that last updated it."""
    out = json.loads(json.dumps(plot))
    out["source_track_key"] = key
    out["position"] = dict(out.get("position", {}), latitude=lat, longitude=lon)
    kin = dict(out.get("kinematics") or {})
    kin["course_deg"] = course
    kin["speed_mps"] = speed
    out["kinematics"] = kin
    if at_ms is not None:
        out["observed_at"] = iso(at_ms)
    if dropped:
        out["state"] = "dropped"
    return out


_log_sink: Callable[[str, str], None] | None = None


def log(level: str, message: str) -> None:
    """A line in OpenTrack's log (in a component), or on stderr (served)."""
    if _log_sink is not None:
        _log_sink(level, message)
    else:
        print(f"[{level}] {message}", file=sys.stderr, flush=True)


def main(plugin: Plugin, argv: list[str] | None = None) -> None:
    """The command line of a plugin module: `serve`, `describe` or `build`."""
    import argparse

    ap = argparse.ArgumentParser(description=plugin.manifest.get("description"))
    sub = ap.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("serve", help="serve the plugin over a socket (an external plugin)")
    s.add_argument("--address", default="127.0.0.1:47300", help="host:port or unix:/path")
    s.add_argument(
        "--secret-file",
        help="a file holding the secret shared with OpenTrack (default: the OT_PLUGIN_SECRET variable)",
    )
    sub.add_parser("describe", help="print the manifest")
    b = sub.add_parser("build", help="build a WebAssembly component (plain Python only)")
    b.add_argument("-o", "--output", required=True)
    args = ap.parse_args(argv)
    if args.cmd == "serve":
        from opentrack_plugin.external import load_secret, serve

        serve(plugin, args.address, load_secret(args.secret_file))
    elif args.cmd == "describe":
        print(json.dumps(plugin.describe(), indent=2))
    else:
        from opentrack_plugin.build import build

        module = sys.modules["__main__"].__file__
        build(module, args.output)

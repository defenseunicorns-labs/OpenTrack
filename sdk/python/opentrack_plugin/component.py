"""A plugin as a WebAssembly component: the classes componentize-py wants
(`Meta`, `Codec`, `Tracker`, `Scorer`), calling the plugin's own. `build`
writes an app module that does

    Meta, Codec, Tracker, Scorer = export(PLUGIN)

so plugin authors never see the generated bindings.
"""

from __future__ import annotations

import json

import opentrack_plugin
from opentrack_plugin import Plugin, PluginError


def export(plugin: Plugin):
    from componentize_py_types import Err
    from wit_world.exports import codec, scorer, tracker
    from wit_world.imports import host

    levels = {
        "debug": host.Level.DEBUG,
        "info": host.Level.INFO,
        "warn": host.Level.WARN,
        "error": host.Level.ERROR,
    }
    opentrack_plugin._log_sink = lambda level, message: host.log(levels.get(level, host.Level.INFO), message)

    def open_(kind: str, options: str):
        try:
            return plugin.open(kind, json.loads(options or "{}"))
        except PluginError as e:
            raise Err(str(e))
        except Exception as e:  # noqa: BLE001 - becomes the call's error
            raise Err(f"{type(e).__name__}: {e}")

    def guard(f):
        try:
            return f()
        except Exception as e:  # noqa: BLE001 - becomes the call's error
            raise Err(str(e) if isinstance(e, PluginError) else f"{type(e).__name__}: {e}")

    class Decoder(codec.Decoder):
        def __init__(self, inner):
            self.inner = inner

        def decode(self, frame: bytes, received_at_ms: int):
            return guard(lambda: [json.dumps(r) for r in self.inner.decode(bytes(frame), received_at_ms)])

        def hints(self):
            h = self.inner.hints()
            return None if h is None else json.dumps(h)

    class TrackerRes(tracker.Tracker):
        def __init__(self, inner):
            self.inner = inner

        def push(self, plot: str, received_at_ms: int) -> None:
            self.inner.push(json.loads(plot), received_at_ms)

        def run(self, now_ms: int, force: bool):
            return guard(lambda: [json.dumps(t) for t in self.inner.run(now_ms, force)])

    class ScorerRes(scorer.Scorer):
        def __init__(self, inner):
            self.inner = inner

        def score(self, report: str, candidates):
            return guard(
                lambda: [
                    json.dumps(s)
                    for s in self.inner.score(json.loads(report), [json.loads(c) for c in candidates])
                ]
            )

    class Meta:
        def describe(self) -> str:
            return json.dumps(plugin.describe())

    class Codec:
        def open_decoder(self, options: str):
            return Decoder(open_("codec", options))

    class Tracker:
        def open_tracker(self, options: str):
            return TrackerRes(open_("tracker", options))

    class Scorer:
        def open_scorer(self, options: str):
            return ScorerRes(open_("scorer", options))

    return Meta, Codec, Tracker, Scorer

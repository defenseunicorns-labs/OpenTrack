"""An example scorer in plain Python (it builds into a WebAssembly component):
OpenTrack's kinematic evidence, but never across domains (a surface track
and an air track are not the same object, however close) and not when their
courses disagree by more than `max_course_diff_deg` while both move.

    python domain_scorer.py build -o domain_scorer.wasm     # a component
    python domain_scorer.py serve --address 127.0.0.1:47301 # or served

Served, it needs the secret it shares with OpenTrack: in OT_PLUGIN_SECRET, or
a file given with --secret-file (Settings -> Plugins makes one).
"""

from opentrack_plugin import Plugin, Scorer, main


def domain(obs):
    return (obs.get("classification") or {}).get("domain")


def moving_course(obs):
    k = obs.get("kinematics") or {}
    if (k.get("speed_mps") or 0) > 1.0:
        return k.get("course_deg")
    return None


class DomainScorer(Scorer):
    def score(self, report, candidates):
        limit = float(self.options["max_course_diff_deg"])
        out = []
        for c in candidates:
            k = c["kinematic"]
            view = c["view"]
            a, b = domain(report), domain(view)
            ca, cb = moving_course(report), moving_course(view)
            diff = None if ca is None or cb is None else abs((ca - cb + 180) % 360 - 180)
            if a and b and a != b:
                out.append({"ln_lr": -20.0, "pass": False, "evidence": {"veto": "domain", "domains": [a, b]}})
            elif diff is not None and diff > limit:
                out.append({"ln_lr": -20.0, "pass": False, "evidence": {"veto": "course", "course_diff_deg": diff}})
            else:
                out.append({"ln_lr": k["ln_lr"], "pass": k["pass"], "evidence": {"course_diff_deg": diff}})
        return out


PLUGIN = Plugin(
    {
        "name": "domain-scorer",
        "version": "1",
        "description": "Example scorer (Python): kinematic evidence, never across domains or disagreeing courses",
        "options": [
            {"name": "max_course_diff_deg", "label": "Largest course difference (°)", "type": "number",
             "default": 60.0, "unit": "°", "min": 0.0,
             "help": "Moving tracks whose courses differ by more than this never pair"},
        ],
    },
    scorer=DomainScorer,
)

if __name__ == "__main__":
    main(PLUGIN)

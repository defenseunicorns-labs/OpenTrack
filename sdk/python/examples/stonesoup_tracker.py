"""An example tracker served as an external plugin: Stone Soup's Kalman
filter with global nearest neighbour association (GNNWith2DAssignment) and
multi-measurement initiation. It needs numpy, scipy and Stone Soup, so it
runs as its own program rather than as WebAssembly:

    pip install stonesoup
    python stonesoup_tracker.py serve --address 127.0.0.1:47310 --secret-file plugin.secret

(the secret it shares with OpenTrack: Settings -> Plugins makes one; or set
OT_PLUGIN_SECRET). Then add 127.0.0.1:47310 with that secret in Settings ->
Plugins and give a source's tracker
stage {"algorithm": "plugin", "plugin": "stonesoup-gnn", "options": {...}}.
Plots with the same time are one scan. Positions are tracked in a flat
east/north frame around the first plot.
"""

import math
from datetime import datetime, timedelta, timezone

import numpy as np
from stonesoup.dataassociator.neighbour import GNNWith2DAssignment
from stonesoup.deleter.time import UpdateTimeDeleter
from stonesoup.hypothesiser.distance import DistanceHypothesiser
from stonesoup.initiator.simple import MultiMeasurementInitiator
from stonesoup.measures import Mahalanobis
from stonesoup.models.measurement.linear import LinearGaussian
from stonesoup.models.transition.linear import CombinedLinearGaussianTransitionModel, ConstantVelocity
from stonesoup.predictor.kalman import KalmanPredictor
from stonesoup.types.detection import Detection
from stonesoup.types.state import GaussianState
from stonesoup.updater.kalman import KalmanUpdater

from opentrack_plugin import Plugin, Tracker, main, observed_ms, track

R = 6378137.0


class StoneSoupGnn(Tracker):
    def __init__(self, options):
        super().__init__(options)
        o = self.options
        q = float(o["process_noise_mps2"]) ** 2
        sigma = float(o["measurement_sigma_m"])
        self.transition = CombinedLinearGaussianTransitionModel([ConstantVelocity(q), ConstantVelocity(q)])
        self.measurement = LinearGaussian(ndim_state=4, mapping=(0, 2), noise_covar=np.diag([sigma**2, sigma**2]))
        predictor = KalmanPredictor(self.transition)
        self.updater = KalmanUpdater(self.measurement)
        hypothesiser = DistanceHypothesiser(predictor, self.updater, Mahalanobis(), missed_distance=float(o["gate"]))
        self.associator = GNNWith2DAssignment(hypothesiser)
        self.deleter = UpdateTimeDeleter(timedelta(seconds=float(o["drop_secs"])))
        v0 = float(o["initial_speed_sigma_mps"])
        prior = GaussianState([[0], [0], [0], [0]], np.diag([0, v0**2, 0, v0**2]))
        self.initiator = MultiMeasurementInitiator(
            prior_state=prior,
            measurement_model=self.measurement,
            deleter=UpdateTimeDeleter(timedelta(seconds=float(o["drop_secs"]))),
            data_associator=GNNWith2DAssignment(hypothesiser),
            updater=self.updater,
            min_points=int(o["confirm_hits"]),
        )
        self.tracks = set()
        self.names = {}
        self.pending = []
        self.origin = None
        self.last_plot = {}
        self.next = 1

    def en(self, lat, lon):
        lat0, lon0 = self.origin
        return math.radians(lon - lon0) * R * math.cos(math.radians(lat0)), math.radians(lat - lat0) * R

    def latlon(self, e, n):
        lat0, lon0 = self.origin
        return lat0 + math.degrees(n / R), lon0 + math.degrees(e / (R * math.cos(math.radians(lat0))))

    def push(self, plot, received_at_ms):
        self.pending.append(plot)

    def run(self, now_ms, force):
        scans = {}
        for p in self.pending:
            scans.setdefault(observed_ms(p), []).append(p)
        self.pending = []
        out = []
        for t_ms in sorted(scans):
            plots = scans[t_ms]
            when = datetime.fromtimestamp(t_ms / 1000, timezone.utc).replace(tzinfo=None)
            if self.origin is None:
                pos = plots[0]["position"]
                self.origin = (pos["latitude"], pos["longitude"])
            detections = set()
            for p in plots:
                e, n = self.en(p["position"]["latitude"], p["position"]["longitude"])
                d = Detection(np.array([[e], [n]]), timestamp=when, measurement_model=self.measurement,
                              metadata={"plot": p})
                detections.add(d)
            associations = self.associator.associate(self.tracks, detections, when)
            used = set()
            for tr, hyp in associations.items():
                if hyp:
                    tr.append(self.updater.update(hyp))
                    used.add(hyp.measurement)
                    self.last_plot[tr] = hyp.measurement.metadata["plot"]
                else:
                    tr.append(hyp.prediction)
            self.tracks |= self.initiator.initiate(detections - used, when)
            for tr in self.tracks:
                if tr not in self.names:
                    self.names[tr] = f"SS{self.next}"
                    self.next += 1
                    if tr not in self.last_plot:
                        self.last_plot[tr] = plots[0]
            gone = self.deleter.delete_tracks(self.tracks)
            self.tracks -= gone
            for tr in self.tracks:
                if tr.state.timestamp == when and hasattr(tr.state, "hypothesis"):
                    out.append(self.report(tr, t_ms))
            for tr in gone:
                out.append(self.report(tr, t_ms, dropped=True))
                self.names.pop(tr, None)
                self.last_plot.pop(tr, None)
        return out

    def report(self, tr, t_ms, dropped=False):
        x = tr.state_vector
        e, ve, n, vn = float(x[0]), float(x[1]), float(x[2]), float(x[3])
        lat, lon = self.latlon(e, n)
        speed = math.hypot(ve, vn)
        course = math.degrees(math.atan2(ve, vn)) % 360 if speed > 0.5 else None
        return track(self.last_plot[tr], self.names[tr], lat, lon, course=course, speed=round(speed, 2),
                     at_ms=t_ms, dropped=dropped)


def option(name, label, default, unit, help_):
    return {"name": name, "label": label, "type": "number", "default": default, "unit": unit, "min": 0.0,
            "help": help_}


PLUGIN = Plugin(
    {
        "name": "stonesoup-gnn",
        "version": "1",
        "description": "Example tracker (external, Python): Stone Soup Kalman filter with GNN association",
        "options": [
            option("measurement_sigma_m", "Plot error σ (m)", 20.0, "m", "Standard deviation of a plot per axis"),
            option("process_noise_mps2", "Process noise (m/s²)", 1.0, "m/s²", "How hard targets manoeuvre"),
            option("initial_speed_sigma_mps", "Initial speed σ (m/s)", 15.0, "m/s", "Velocity uncertainty of a new track"),
            option("gate", "Gate (Mahalanobis)", 3.0, "", "Plots further than this from a prediction do not update it"),
            option("confirm_hits", "Confirm after", 3.0, "", "Plots before a track is reported"),
            option("drop_secs", "Drop after (s)", 20.0, "s", "Seconds without a plot before a track ends"),
        ],
    },
    tracker=StoneSoupGnn,
)

if __name__ == "__main__":
    main(PLUGIN)

"""A simulated acoustic array: a fixed microphone array that hears a small
UAS and reports a bearing (and, often, a slant range) with noise, in the
manner of a SAPIENT acoustic node (`docs/examples/sapient-acoustic-mqtt.json`,
`docs/non-point-contacts.md` "Acoustic arrays"). Unlike the radar model
(`radar.py`), detections are not scanned in a batch: each array reports
(or misses) every object within range on its own clock, independently.

Parameters are cited from public, open figures, kept modest and rounded:

- detection range ~300-500 m for a small quadcopter-class UAS: counter-UAS
  acoustic sensor literature (e.g. Squarehead Discovair, DroneShield acoustic
  module public specifications quote a few hundred metres for small
  multirotors, far less than for radar or RF detection).
- bearing (direction-finding) accuracy ~2-5 deg one sigma for a small fixed
  array, consistent with published acoustic DF accuracy for compact arrays.
- report rate ~1 Hz, typical of an acoustic detection track update rate.
- small-UAS speed ~5-20 m/s (a DJI Mavic-class quadcopter cruises around
  10 m/s and tops out near 20 m/s; a loitering or orbiting drone is slower).

A report without a range is a line of bearing; `ranged_share` of in-range
reports also carries a range estimate (noisy slant range), which OpenTrack
converts straight to a point with its error ellipse. `false_rate_hz` adds a
modest rate of spurious, anonymous bearings (acoustic false alarms: wind,
traffic, other aircraft), each its own one-off key so it never forms a
persistent track by itself.
"""

import math
import random

from common import Local


class AcousticArray:
    def __init__(
        self,
        name: str,
        lat: float,
        lon: float,
        alt: float,
        *,
        range_m: float = 400.0,
        sigma_bearing_deg: float = 3.0,
        sigma_elevation_deg: float = 2.0,
        pd: float = 0.9,
        ranged_share: float = 0.6,
        range_sigma_frac: float = 0.03,
        range_sigma_min_m: float = 3.0,
        false_rate_hz: float = 0.01,
        seed: int = 1,
    ):
        self.name = name
        self.lat, self.lon, self.alt = lat, lon, alt
        self.local = Local(lat, lon)
        self.range_m = range_m
        self.sigma_bearing = math.radians(sigma_bearing_deg)
        self.sigma_bearing_deg = sigma_bearing_deg
        self.sigma_elevation_deg = sigma_elevation_deg
        self.pd = pd
        self.ranged_share = ranged_share
        self.range_sigma_frac = range_sigma_frac
        self.range_sigma_min_m = range_sigma_min_m
        self.false_rate_hz = false_rate_hz
        self.rng = random.Random(seed)
        self._false_n = 0

    def settings(self) -> dict:
        return {
            "site": [round(self.lat, 6), round(self.lon, 6), self.alt],
            "range_m": self.range_m,
            "sigma_bearing_deg": self.sigma_bearing_deg,
            "sigma_elevation_deg": self.sigma_elevation_deg,
            "pd": self.pd,
            "ranged_share": self.ranged_share,
            "false_rate_hz": self.false_rate_hz,
        }

    def covers(self, lat: float, lon: float) -> bool:
        e, n = self.local.en(lat, lon)
        return math.hypot(e, n) <= self.range_m

    def detect(self, key: str, lat: float, lon: float, alt: float):
        """One object's report this second, or None (missed or out of
        range). `key` is this array's own numbering for the object (never
        an identifier: see `docs/non-point-contacts.md`)."""
        e, n = self.local.en(lat, lon)
        d = math.hypot(e, n)
        if d > self.range_m or self.rng.random() > self.pd:
            return None
        bearing = math.degrees(math.atan2(e, n)) % 360.0
        slant = math.hypot(d, alt - self.alt)
        elevation = math.degrees(math.atan2(alt - self.alt, max(d, 1.0)))
        bearing_meas = (bearing + self.rng.gauss(0, self.sigma_bearing_deg)) % 360.0
        elevation_meas = elevation + self.rng.gauss(0, self.sigma_elevation_deg)
        ranged = self.rng.random() < self.ranged_share
        range_sigma = max(self.range_sigma_frac * slant, self.range_sigma_min_m)
        range_meas = slant + self.rng.gauss(0, range_sigma) if ranged else None
        return {
            "id": key,
            "bearing": round(bearing_meas, 2),
            "sigma": self.sigma_bearing_deg,
            "elevation": round(elevation_meas, 2),
            "range": round(range_meas, 1) if ranged else None,
            "range_sigma": round(range_sigma, 1) if ranged else None,
        }

    def false_detection(self):
        """A spurious, anonymous bearing this second, or None. Its own
        one-off key: a real array's false alarm is rarely repeated in the
        same direction, so this never forms a persistent clutter track."""
        if self.rng.random() > self.false_rate_hz:
            return None
        self._false_n += 1
        return {
            "id": f"x{self._false_n}",
            "bearing": round(self.rng.uniform(0, 360), 2),
            "sigma": self.sigma_bearing_deg,
            "elevation": round(self.rng.uniform(-5, 25), 2),
            "range": None,
            "range_sigma": None,
        }


def acoustic_spec(array: AcousticArray, array_id: str, port: int, *,
                   manoeuvre_mps2: float = 5.0, max_speed_mps: float = 20.0) -> dict:
    """A source spec for one acoustic array: a plain JSON codec carrying the
    same fields the real SAPIENT mapping produces (bearing, sigma, range,
    range_sigma, elevation), so the engine's real mapping, tracker-free
    "reports: tracks" path and correlation run exactly as they would on the
    real feed (`docs/examples/sapient-acoustic-mqtt.json`); only the wire
    format (SAPIENT protobuf over MQTT) is replaced by a JSON frame, as the
    other benchmark scenarios replace UDP radar feeds with JSON plots."""
    return {
        "id": array_id,
        "name": f"Simulated acoustic array {array_id}",
        "description": "Synthetic acoustic array: bearing (+ often range) reports to objects it hears, "
                       "its own numbering per object (docs/non-point-contacts.md, Acoustic arrays).",
        "transport": {"type": "udp", "bind": f"127.0.0.1:{port}"},
        "unauthenticated": "accepted",
        "emitter_motion": {"manoeuvre_mps2": manoeuvre_mps2, "max_speed_mps": max_speed_mps},
        "pipeline": {
            "codec": {"type": "json", "records": "detections"},
            "mapping": {
                "schema_version": 2,
                "rules": [
                    {
                        "name": "bearing",
                        "key": "id",
                        "fields": {
                            "observed_at": {"path": "t", "transforms": ["time"]},
                            "position.latitude": {"const": round(array.lat, 7)},
                            "position.longitude": {"const": round(array.lon, 7)},
                            "position.altitude_hae_m": {"const": array.alt},
                            "geometry.bearing_deg": "bearing",
                            "geometry.sigma_deg": "sigma",
                            "geometry.range_m": "range",
                            "geometry.range_sigma_m": "range_sigma",
                            "geometry.max_range_m": {"const": array.range_m},
                            "geometry.elevation_deg": "elevation",
                            "classification.domain": {"const": "air"},
                            "track_type": {"const": "simulated_training"},
                        },
                    }
                ],
            },
        },
    }


def acoustic_frames(w, arrays: dict, drones: dict, t0: float, duration: float, step: float = 1.0):
    """Feed every array's detections of every drone (and its own false
    alarms), one frame per array per step. `arrays`: {array_id: (AcousticArray,
    array_object_index_unused)}. `drones`: {drone_id: path(t) -> (lat, lon,
    alt) or None (out of the scenario entirely)}. Returns {drone_id:
    [(t, lat, lon, alt, scored)]} for the truth writer, `scored` False where
    no array covers the point."""
    truth = {d: [] for d in drones}
    n = int(duration / step) + 1
    for i in range(n):
        t = t0 + i * step
        for array_id, array in arrays.items():
            records = []
            for drone_id, path in drones.items():
                p = path(t)
                if p is None:
                    continue
                lat, lon, alt = p
                r = array.detect(f"{array_id}-{drone_id}", lat, lon, alt)
                if r is not None:
                    r["t"] = t
                    records.append(r)
            fr = array.false_detection()
            if fr is not None:
                fr["t"] = t
                records.append(fr)
            if records:
                w.frame(array_id, t, json_={"detections": records})
        for drone_id, path in drones.items():
            p = path(t)
            if p is None:
                continue
            lat, lon, alt = p
            scored = any(a.covers(lat, lon) for a in arrays.values())
            truth[drone_id].append((t, lat, lon, alt, scored))
    return truth

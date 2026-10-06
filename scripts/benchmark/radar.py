"""A simulated surveillance radar, in the manner of Stone Soup's
RadarBearingRange on a fixed platform: each scan detects every target in
coverage with probability `pd`, with Gaussian range and bearing noise, and
adds Poisson clutter spread uniformly over the coverage.

A scan is one frame of plots at one time (as a rotating radar's tracker
input usually is: one plot list per revolution). Every plot keeps the target
it came from (None for clutter), so scoring can check the trackers.
"""

import math
import random

from common import Local


class Radar:
    def __init__(
        self,
        name: str,
        lat: float,
        lon: float,
        *,
        revisit_s: float = 4.0,
        range_max_m: float = 60_000.0,
        range_min_m: float = 200.0,
        sigma_range_m: float = 20.0,
        sigma_bearing_deg: float = 0.3,
        pd: float = 0.9,
        clutter_per_scan: float = 5.0,
        min_alt_m: float | None = None,
        max_alt_m: float | None = None,
        seed: int = 1,
    ):
        self.name = name
        self.lat, self.lon = lat, lon
        self.local = Local(lat, lon)
        self.revisit_s = revisit_s
        self.range_max_m, self.range_min_m = range_max_m, range_min_m
        self.sigma_range_m = sigma_range_m
        self.sigma_bearing = math.radians(sigma_bearing_deg)
        self.pd = pd
        self.clutter_per_scan = clutter_per_scan
        self.min_alt_m, self.max_alt_m = min_alt_m, max_alt_m
        self.rng = random.Random(seed)

    def settings(self) -> dict:
        return {
            "site": [round(self.lat, 5), round(self.lon, 5)],
            "revisit_s": self.revisit_s,
            "range_m": [self.range_min_m, self.range_max_m],
            "sigma_range_m": self.sigma_range_m,
            "sigma_bearing_deg": round(math.degrees(self.sigma_bearing), 3),
            "pd": self.pd,
            "clutter_per_scan": self.clutter_per_scan,
            "alt_m": [self.min_alt_m, self.max_alt_m],
        }

    def covers(self, lat: float, lon: float, alt: float | None = None) -> bool:
        e, n = self.local.en(lat, lon)
        r = math.hypot(e, n)
        if not self.range_min_m <= r <= self.range_max_m:
            return False
        if alt is not None:
            if self.min_alt_m is not None and alt < self.min_alt_m:
                return False
            if self.max_alt_m is not None and alt > self.max_alt_m:
                return False
        return True

    def _poisson(self, lam: float) -> int:
        # Knuth; clutter means are small.
        limit, k, p = math.exp(-lam), 0, 1.0
        while True:
            p *= self.rng.random()
            if p <= limit:
                return k
            k += 1

    def _plot(self, r: float, b: float, truth):
        e, n = r * math.sin(b), r * math.cos(b)
        lat, lon = self.local.ll(e, n)
        cross = max(r * self.sigma_bearing, 1.0)
        major, minor = max(cross, self.sigma_range_m), min(cross, self.sigma_range_m)
        # The long axis is across the beam when bearing error dominates.
        orient = (math.degrees(b) + (90.0 if cross >= self.sigma_range_m else 0.0)) % 180
        return {
            "lat": round(lat, 7),
            "lon": round(lon, 7),
            "smaj": round(major, 1),
            "smin": round(minor, 1),
            "orient": round(orient, 1),
            "truth": truth,
        }

    def scan(self, targets):
        """targets: [(id, lat, lon, alt or None)] at the scan time; the plots."""
        plots = []
        for tid, lat, lon, alt in targets:
            if not self.covers(lat, lon, alt) or self.rng.random() > self.pd:
                continue
            e, n = self.local.en(lat, lon)
            r = math.hypot(e, n) + self.rng.gauss(0, self.sigma_range_m)
            b = math.atan2(e, n) + self.rng.gauss(0, self.sigma_bearing)
            plots.append(self._plot(r, b, tid))
        for _ in range(self._poisson(self.clutter_per_scan)):
            # Uniform over the annulus.
            r = math.sqrt(self.rng.uniform(self.range_min_m**2, self.range_max_m**2))
            b = self.rng.uniform(0, 2 * math.pi)
            plots.append(self._plot(r, b, None))
        self.rng.shuffle(plots)
        return plots

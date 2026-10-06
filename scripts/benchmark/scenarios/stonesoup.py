"""The recorded data of Stone Soup's demos (dstl/Stone-Soup, MIT), with
simulated radars in the manner of its "Tracking Groundtruth ADS-B Data by
Simulating Radar Detections" demo: https://stonesoup.readthedocs.io/en/latest/auto_demos/

- `solent-radar`: AIS from the Solent (12 Jan 2016, 85 min, 90 vessels) as
  truth; a coastal radar at Southsea sees them.
- `solent-fusion`: the same radar, plus the AIS itself as a track feed.
- `opensky-radar`: ADS-B states from OpenSky over England (20 min, 84
  aircraft) as truth; air surveillance radars at Heathrow and Manchester (the
  demo's sites) see them.
- `opensky-fusion`: the same radars, plus the ADS-B as a track feed.

Truth between reports is interpolated (gaps up to MAX_GAP_S); points no radar
covers are don't-care.
"""

import csv
import math
from datetime import datetime, timezone

from common import Writer, fetch, iso, raw_dir, spec
from radar import Radar

URL = "https://raw.githubusercontent.com/dstl/Stone-Soup/main/docs/demos/{}"


def interpolate(reports, step, max_gap):
    """reports: {id: [(t, lat, lon, alt)]} → per time step [(id, lat, lon, alt)]."""
    series = {k: sorted(v) for k, v in reports.items() if len(v) >= 2}
    if not series:
        return {}
    t0 = min(s[0][0] for s in series.values())
    t1 = max(s[-1][0] for s in series.values())
    out = {}
    idx = {k: 0 for k in series}
    t = math.ceil(t0 / step) * step
    while t <= t1:
        at = []
        for k, s in series.items():
            i = idx[k]
            while i + 1 < len(s) and s[i + 1][0] < t:
                i += 1
            idx[k] = i
            if i + 1 >= len(s):
                continue
            (ta, la, oa, aa), (tb, lb, ob, ab) = s[i], s[i + 1]
            if not ta <= t <= tb or tb - ta > max_gap:
                continue
            f = (t - ta) / (tb - ta) if tb > ta else 0.0
            alt = None if aa is None or ab is None else aa + f * (ab - aa)
            at.append((k, la + f * (lb - la), oa + f * (ob - oa), alt))
        out[t] = at
        t += step
    return out


def radar_frames(w, radars, truth_at, step):
    """Scan every radar on its revisit; plots go to the radar's source."""
    times = sorted(truth_at)
    for r in radars:
        next_scan = times[0]
        for n, t in enumerate(times):
            if t < next_scan:
                continue
            next_scan = t + r.revisit_s
            plots = []
            for i, p in enumerate(r.scan(truth_at[t])):
                plots.append({"id": f"{int(t)}-{i}", "t": iso(t), "lat": p["lat"], "lon": p["lon"],
                              "smaj": p["smaj"], "smin": p["smin"], "orient": p["orient"]})
            w.frame(r.name, t, json_={"plots": plots})


def radar_spec(r, domain, tracker):
    s = spec("radar-plots", id=r.name, name=f"Simulated radar {r.name}")
    s["pipeline"]["tracker"] = dict(tracker, domain=domain, key_prefix=r.name[:1].upper())
    return s


def truth_points(w, radars, truth_at, step_every):
    for n, (t, targets) in enumerate(sorted(truth_at.items())):
        if n % step_every:
            continue
        for k, lat, lon, alt in targets:
            w.truth_point(t, k, lat, lon, alt, score=any(r.covers(lat, lon, alt) for r in radars))


# --- Solent AIS ---

KNOT = 0.514444


def solent_reports():
    path = fetch(URL.format("SolentAIS_20160112_130211.csv"), raw_dir() / "stonesoup/SolentAIS_20160112_130211.csv")
    rows = []
    for r in csv.DictReader(open(path)):
        lat, lon = float(r["Latitude_degrees"]), float(r["Longitude_degrees"])
        if not (50.4 < lat < 51.0 and -2.0 < lon < 0.0):
            continue  # a few reports are far outside the Solent
        t = datetime.fromisoformat(r["Time"]).replace(tzinfo=timezone.utc).timestamp()
        rows.append((t, r["MMSI"], lat, lon, float(r["COG_degrees"] or 0), float(r["SOG_knots"] or 0) * KNOT))
    return rows


def solent(fusion):
    name = "solent-" + ("fusion" if fusion else "radar")
    print(name)
    rows = solent_reports()
    reports = {}
    for t, mmsi, lat, lon, _, _ in rows:
        reports.setdefault(mmsi, []).append((t, lat, lon, None))
    truth_at = interpolate(reports, 1.0, 180.0)
    radar = Radar("coastal", 50.7780, -1.0880, revisit_s=3.0, range_max_m=24_000, range_min_m=150,
                  sigma_range_m=15.0, sigma_bearing_deg=0.5, pd=0.85, clutter_per_scan=10.0, seed=11)
    w = Writer(name)
    radar_frames(w, [radar], truth_at, 1.0)
    truth_points(w, [radar], truth_at, 5)
    sources = [radar_spec(radar, "surface", {"algorithm": "mht", "measurement_sigma_m": 60.0,
                                              "clutter_density": 2e-8})]
    if fusion:
        feed = spec("track-feed", id="ais", name="Solent AIS")
        feed["pipeline"]["mapping"]["rules"][0]["identifiers"] = [{"scheme": "mmsi", "value": "id"}]
        sources.insert(0, feed)
        for t, mmsi, lat, lon, cog, sog in rows:
            w.frame("ais", t, json_={"id": mmsi, "t": iso(t), "lat": lat, "lon": lon, "cep": 10.0,
                                     "course": cog if sog > 0.3 else None, "speed": round(sog, 2),
                                     "domain": "surface"})
    w.finish(
        "The Solent, 12 Jan 2016: AIS as truth; a simulated coastal radar at Southsea"
        + (", with the AIS as a track feed." if fusion else "."),
        sources,
        engine={"drop_after_secs": 300},
        sample_secs=5.0,
        notes={"dataset": "Stone Soup demo data (dstl/Stone-Soup, MIT)", "truth_complete": True,
               "gospa_cutoff_m": 500, "radar": radar.settings()},
    )


# --- OpenSky ADS-B ---

def opensky_reports():
    path = fetch(URL.format("OpenSky_Plane_States.csv"), raw_dir() / "stonesoup/OpenSky_Plane_States.csv")
    rows = []
    for r in csv.DictReader(open(path)):
        if not r["lat"] or not r["lon"] or r["onground"] == "True":
            continue
        alt = r["geoaltitude"] or r["baroaltitude"]
        rows.append((float(r["time"]), r["icao24"], float(r["lat"]), float(r["lon"]),
                     float(alt) if alt else None, r["callsign"].strip() or None,
                     float(r["heading"]) if r["heading"] else None,
                     float(r["velocity"]) if r["velocity"] else None))
    return rows


def opensky(fusion):
    name = "opensky-" + ("fusion" if fusion else "radar")
    print(name)
    rows = opensky_reports()
    reports = {}
    for t, icao, lat, lon, alt, *_ in rows:
        reports.setdefault(icao, []).append((t, lat, lon, alt))
    truth_at = interpolate(reports, 1.0, 60.0)
    common = dict(revisit_s=4.0, range_max_m=110_000, range_min_m=500, sigma_range_m=50.0,
                  sigma_bearing_deg=0.15, pd=0.9, clutter_per_scan=5.0, min_alt_m=150.0)
    radars = [Radar("heathrow", 51.4700, -0.4543, seed=21, **common),
              Radar("manchester", 53.3537, -2.2750, seed=22, **common)]
    w = Writer(name)
    radar_frames(w, radars, truth_at, 1.0)
    truth_points(w, radars, truth_at, 2)
    tracker = {"algorithm": "mht", "measurement_sigma_m": 150.0, "process_noise_mps2": 3.0,
               "initial_speed_sigma_mps": 150.0, "clutter_density": 1e-10}
    sources = [radar_spec(r, "air", tracker) for r in radars]
    if fusion:
        feed = spec("track-feed", id="adsb", name="OpenSky ADS-B")
        feed["pipeline"]["mapping"]["rules"][0]["identifiers"] = [{"scheme": "icao24", "value": "id"}]
        sources.insert(0, feed)
        for t, icao, lat, lon, alt, callsign, heading, velocity in rows:
            w.frame("adsb", t, json_={"id": icao, "name": callsign, "t": iso(t), "lat": lat, "lon": lon,
                                      "alt": alt, "course": heading, "speed": velocity, "cep": 30.0,
                                      "domain": "air"})
    w.finish(
        "England, 12 Jul 2021: OpenSky ADS-B as truth; simulated air surveillance radars at Heathrow and "
        "Manchester" + (", with the ADS-B as a track feed." if fusion else "."),
        sources,
        engine={"drop_after_secs": 120, "stale_air_secs": 30},
        sample_secs=5.0,
        notes={"dataset": "Stone Soup demo data (dstl/Stone-Soup, MIT)", "truth_complete": True,
               "gospa_cutoff_m": 2000, "radars": [r.settings() for r in radars]},
    )


SCENARIOS = {
    "solent-radar": lambda: solent(False),
    "solent-fusion": lambda: solent(True),
    "opensky-radar": lambda: opensky(False),
    "opensky-fusion": lambda: opensky(True),
}


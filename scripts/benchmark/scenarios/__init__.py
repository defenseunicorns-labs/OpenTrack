"""Every benchmark scenario, by name: name -> function that builds it."""

from scenarios import acoustic, ferry, gmti, load, stonesoup, synthetic

SCENARIOS = {}
for _m in (ferry, stonesoup, synthetic, gmti, load, acoustic):
    SCENARIOS.update(_m.SCENARIOS)

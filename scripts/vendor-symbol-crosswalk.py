#!/usr/bin/env python3
"""Vendor the MIL-STD-2525D -> 2525C symbol crosswalk the symbol designer writes CoT types from.

Source: Esri/joint-military-symbology-xml (JMSML, Apache-2.0),
samples/legacy_support/LegacyMappingTableCtoD.csv at a pinned commit. A Cursor-on-Target type is
the 2525C warfighting SIDC spelled out (SFGPUCI -> a-f-G-U-C-I), so for each 2525D point symbol
(symbol set + 6-digit entity code, no modifiers) this keeps its 2525C battle dimension and function
ID: {"10121100": "G-UCI", ...}. Writes ui/src/lib/milsym/crosswalk.json.

Usage: scripts/vendor-symbol-crosswalk.py
"""
import csv
import io
import json
import pathlib
import urllib.request

COMMIT = "155b41a04ee9cf62de30906360b9c0fb9f2167f2"
URL = (
    "https://raw.githubusercontent.com/Esri/joint-military-symbology-xml/"
    f"{COMMIT}/samples/legacy_support/LegacyMappingTableCtoD.csv"
)
OUT = pathlib.Path(__file__).resolve().parent.parent / "ui/src/lib/milsym/crosswalk.json"


def main() -> None:
    text = urllib.request.urlopen(URL, timeout=60).read().decode("utf-8-sig")
    out: dict[str, str] = {}
    for row in csv.DictReader(io.StringIO(text)):
        if row["2525DeltaMod1"] != "00" or row["2525DeltaMod2"] != "00":
            continue
        charlie = (row["DeltaToCharlie"] or row["2525Charlie"]).strip().upper()
        # Warfighting symbols only (S…): CoT atoms have no tactical-graphic or SIGINT form.
        if len(charlie) < 10 or charlie[0] != "S":
            continue
        key = row["2525DeltaSymbolSet"] + row["2525DeltaEntity"]
        function = charlie[4:10].rstrip("-")
        out.setdefault(key, f"{charlie[2]}-{function}" if function else charlie[2])
    OUT.write_text(json.dumps(dict(sorted(out.items())), separators=(",", ":")) + "\n")
    print(f"{len(out)} 2525D symbols -> {OUT.relative_to(OUT.parents[4])} (JMSML@{COMMIT[:12]})")


if __name__ == "__main__":
    main()

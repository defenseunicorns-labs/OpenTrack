#!/usr/bin/env bash
# Vendor the map's label font as glyph PBFs into ui/public/map-fonts/ (served at /map-fonts/).
#
# MapLibre draws text from signed-distance-field glyph ranges ({fontstack}/{start}-{end}.pbf).
# OpenTrack serves them itself (no network at run time, and the content security policy allows
# only this origin). Source: protomaps/basemaps-assets at the commit OpenStare pins
# (scripts/basemap/basemap-assets.lock), Noto Sans Medium, SIL Open Font License 1.1.
#
# Usage: scripts/vendor-map-glyphs.sh
set -euo pipefail

commit=028c18f713baecad011301ff7a69acc39bcc2ae7
fontstack="Noto Sans Medium"
root="$(cd "$(dirname "$0")/.." && pwd)"
out="$root/ui/public/map-fonts"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

curl -sfL -o "$work/assets.tar.gz" "https://codeload.github.com/protomaps/basemaps-assets/tar.gz/$commit"
tar -xzf "$work/assets.tar.gz" -C "$work"
src="$work/basemaps-assets-$commit/fonts"

rm -rf "$out"
mkdir -p "$out"
cp -R "$src/$fontstack" "$out/$fontstack"
cp "$src/OFL.txt" "$out/OFL.txt"
echo "$fontstack glyphs from protomaps/basemaps-assets@$commit (SIL OFL 1.1, see OFL.txt)" > "$out/SOURCE"
echo "vendored $(ls "$out/$fontstack" | wc -l) glyph ranges of $fontstack into ui/public/map-fonts"

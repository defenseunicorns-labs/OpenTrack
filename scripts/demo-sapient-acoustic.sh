#!/usr/bin/env bash
# Configure five OpenTrack MQTT sources and run sapient-sim's acoustic experiment.
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
OT_HOST="${OT_HOST:-127.0.0.1:8090}"
OT_BIN="${OT_BIN:-$ROOT_DIR/target/debug/opentrack}"
SIM_ROOT="${SIM_ROOT:-$ROOT_DIR/../sapient-sim}"
SIM_BIN="${SIM_BIN:-$SIM_ROOT/target/debug/sapient-acoustic-experiment}"
SIM_SCENARIO="${SIM_SCENARIO:-$SIM_ROOT/scenarios/acoustic-five-node.yml}"
DATA_DIR="${DATA_DIR:-$ROOT_DIR/data-sapient-acoustic}"
SOURCE_SPEC="$ROOT_DIR/docs/examples/sapient-acoustic-mqtt.json"
ADMIN_EMAIL="${OT_ADMIN_EMAIL:-admin@opentrack.local}"
export OT_SQLITE_PATH="$DATA_DIR/opentrack.db"

if [ ! -x "$OT_BIN" ]; then
  printf 'ERROR: OpenTrack binary not found at %s\nBuild it with: cargo build --no-default-features\n' "$OT_BIN" >&2
  exit 1
fi
if [ ! -x "$SIM_BIN" ]; then
  printf 'ERROR: acoustic simulator not found at %s\nBuild it with: cargo build --manifest-path "%s/Cargo.toml" -p sapient-sim --bin sapient-acoustic-experiment\n' "$SIM_BIN" "$SIM_ROOT" >&2
  exit 1
fi
if [ ! -r "$SIM_SCENARIO" ]; then
  printf 'ERROR: acoustic scenario not found at %s\n' "$SIM_SCENARIO" >&2
  exit 1
fi
command -v jq >/dev/null || { printf 'ERROR: jq is required\n' >&2; exit 1; }
mkdir -p "$DATA_DIR"

# aws-lc-fips builds a shared crypto library on macOS without embedding an
# rpath in debug binaries. Make the Cargo build artifact visible at runtime.
if [ "$(uname -s)" = Darwin ] && [ -z "${DYLD_LIBRARY_PATH:-}" ]; then
  for candidate in "$ROOT_DIR"/target/debug/build/aws-lc-fips-sys-*/out/build/artifacts; do
    if [ -d "$candidate" ]; then
      export DYLD_LIBRARY_PATH="$candidate"
      break
    fi
  done
fi

OT_PID=""
if ! curl -sf "http://$OT_HOST/healthz" >/dev/null 2>&1; then
  OT_BIND="$OT_HOST" \
    OT_REDIS_URL="redis://127.0.0.1:6379" \
    OT_NATS_URL="nats://127.0.0.1:4222" \
    OT_LOG=info \
    "$OT_BIN" all &
  OT_PID=$!
  printf 'OpenTrack PID: %s\n' "$OT_PID"
fi

printf 'Waiting for OpenTrack API at %s...\n' "$OT_HOST"
for _ in $(seq 1 30); do
  curl -sf "http://$OT_HOST/healthz" >/dev/null 2>&1 && break
  if [ -n "$OT_PID" ] && ! kill -0 "$OT_PID" 2>/dev/null; then
    wait "$OT_PID" || true
    printf 'ERROR: OpenTrack exited before becoming healthy\n' >&2
    exit 1
  fi
  sleep 1
done
curl -sf "http://$OT_HOST/healthz" >/dev/null || {
  printf 'ERROR: OpenTrack did not become healthy\n' >&2
  exit 1
}

TOKEN="$($OT_BIN user token "$ADMIN_EMAIL" --name sapient-acoustic-demo --days 1 | tail -n1)"
AUTH=(-H "Authorization: Bearer $TOKEN")

sensor_ids=(
  00000000-0000-4000-8000-000000000001
  00000000-0000-4000-8000-000000000002
  00000000-0000-4000-8000-000000000003
  00000000-0000-4000-8000-000000000004
  00000000-0000-4000-8000-000000000005
)
latitudes=(51.5000 51.5000 51.5025 51.4975 51.5000)
longitudes=(-0.1020 -0.0980 -0.1000 -0.1000 -0.1000)
altitudes=(18 20 16 22 19)

printf 'Configuring five acoustic array sources...\n'
for i in 0 1 2 3 4; do
  n=$((i + 1))
  source_id="sapient-acoustic-$n"
  body="$(jq \
    --arg id "$source_id" \
    --arg name "SAPIENT acoustic array $n" \
    --arg topic "sapient/v2/detections/acoustic/${sensor_ids[$i]}" \
    --argjson lat "${latitudes[$i]}" \
    --argjson lon "${longitudes[$i]}" \
    --argjson alt "${altitudes[$i]}" \
    '.id = $id
     | .name = $name
     | .transport.topics = [$topic]
     | .pipeline.mapping.rules[0].fields["position.latitude"].const = $lat
     | .pipeline.mapping.rules[0].fields["position.longitude"].const = $lon
     | .pipeline.mapping.rules[0].fields["position.altitude_hae_m"].const = $alt' \
    "$SOURCE_SPEC")"

  curl -sf "${AUTH[@]}" -X PUT "http://$OT_HOST/api/v1/sources/$source_id" \
    -H 'Content-Type: application/json' \
    -d "$body" >/dev/null
  curl -sf "${AUTH[@]}" -X POST "http://$OT_HOST/api/v1/sources/$source_id/enable" \
    >/dev/null
done

printf 'Starting acoustic experiment...\n'
"$SIM_BIN" \
  --scenario "$SIM_SCENARIO" \
  --broker mqtt://127.0.0.1:1883 \
  --duration "${SAPIENT_DURATION:-60s}" &
SIM_PID=$!
printf 'Simulator PID: %s\nOpenTrack UI: http://%s\n' "$SIM_PID" "$OT_HOST"

while kill -0 "$SIM_PID" 2>/dev/null; do
  sleep 5
  printf '%s' "$(date +%T)"
  for n in 1 2 3 4 5; do
    plots="$(curl -sf "${AUTH[@]}" "http://$OT_HOST/api/v1/sources/sapient-acoustic-$n" \
      | jq -r '.status.totals_since_start.plots // 0')"
    printf '  array-%s=%s' "$n" "$plots"
  done
  printf '\n'
done
wait "$SIM_PID"

printf 'Tracks: http://%s/api/v1/tracks\n' "$OT_HOST"

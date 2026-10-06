#!/usr/bin/env bash
# Everything CI checks, locally. Exits non-zero on the first failure.
# Set OT_TEST_REDIS_URL, OT_TEST_NATS_URL and OT_TEST_MQTT_URL to include the
# tests against real servers:
#   docker run -p 4222:4222 nats:2 -js
#   docker run -p 1883:1883 eclipse-mosquitto:2 mosquitto -c /mosquitto-no-auth.conf
set -euo pipefail
cd "$(dirname "$0")/.."
cargo fmt --all --check
cargo clippy --quiet --all-targets --locked -- -D warnings
cargo test --quiet --locked
(cd ui && npm run --silent lint && npm run --silent test && npm run --silent build >/dev/null)
echo "all checks passed"

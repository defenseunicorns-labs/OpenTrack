#!/usr/bin/env bash
# Test coverage of the Rust workspace (cargo-llvm-cov), for the summary kept
# with each release (docs/security/scm-plan.md). Writes
#   target/coverage/summary.txt   per-file and total line, region and function coverage
# and prints the total. Reports are not committed.
#
# Needs: cargo install --locked cargo-llvm-cov
#        rustup component add llvm-tools-preview   (for the pinned toolchain)
# and, for the tests against real servers, Redis, NATS (JetStream) and MQTT;
# point these at them (the defaults are the test containers' ports):
#   OT_TEST_REDIS_URL  OT_TEST_NATS_URL  OT_TEST_MQTT_URL
#
# The UI: `vitest --coverage` needs a coverage provider (@vitest/coverage-v8),
# which is not a dependency of the UI. When one is installed in ui/node_modules
# the script runs it too; otherwise it says so and skips the UI.
set -euo pipefail
cd "$(dirname "$0")/.."

export OT_TEST_REDIS_URL="${OT_TEST_REDIS_URL:-redis://127.0.0.1:6398}"
export OT_TEST_NATS_URL="${OT_TEST_NATS_URL:-nats://127.0.0.1:14222}"
export OT_TEST_MQTT_URL="${OT_TEST_MQTT_URL:-mqtt://127.0.0.1:11883}"

if ! cargo llvm-cov --version >/dev/null 2>&1; then
  echo "cargo-llvm-cov is not installed: cargo install --locked cargo-llvm-cov" >&2
  exit 1
fi

out=target/coverage
mkdir -p "$out"
cargo llvm-cov --workspace --locked --summary-only | tee "$out/summary.txt"
echo
awk '/^TOTAL/ {print "Rust: lines " $10 " of " $8 ", functions " $7 " of " $5 ", regions " $4 " of " $2}' "$out/summary.txt"

if [ -d ui/node_modules/@vitest/coverage-v8 ] || [ -d ui/node_modules/@vitest/coverage-istanbul ]; then
  (cd ui && npx vitest run --coverage --coverage.reporter=text-summary) | tee "$out/ui-summary.txt"
else
  echo "UI: skipped (no @vitest/coverage-v8 in ui/node_modules; not a UI dependency)"
fi

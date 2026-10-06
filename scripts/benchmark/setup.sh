#!/bin/sh
# A Python environment for the benchmark (numpy, scipy) in scripts/benchmark/.venv.
set -e
cd "$(dirname "$0")"
if ! python3 -m venv .venv 2>/dev/null; then
    rm -rf .venv
    python3 -m pip install -q --user --break-system-packages virtualenv
    python3 -m virtualenv -q .venv
fi
.venv/bin/pip install -q -r requirements.txt
echo "ready: scripts/benchmark/bench <command>"

#!/usr/bin/env bash
# Build the app bundle (so TCC grants stay stable), then run Python
# integration tests against the bundled binary.
# Usage: scripts/test.sh [test_*.py]
set -euo pipefail

cd "$(dirname "$0")/.."

scripts/build-app.sh >/dev/null

export CUA_DRIVER_BINARY="$(pwd)/.build/CuaDriver.app/Contents/MacOS/cua-driver"

cd Tests/integration

if [ $# -gt 0 ]; then
    python3 -m unittest "$@"
else
    python3 -m unittest discover -v -p "test_*.py"
fi

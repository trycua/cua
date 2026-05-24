#!/usr/bin/env bash
# Build the app bundle (so TCC grants stay stable), then run Python
# integration tests against the bundled binary.
# Usage: scripts/test.sh [test_*.py]
set -euo pipefail

# scripts/ is cross-cutting; Swift sources + Tests/ moved to swift/.
# Land in swift/ so .build/CuaDriver.app and Tests/integration resolve
# relative to the Swift port.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR/../swift"

"$SCRIPT_DIR/build-app.sh" >/dev/null

export CUA_DRIVER_BINARY="$(pwd)/.build/CuaDriver.app/Contents/MacOS/cua-driver"

cd Tests/integration

if [ $# -gt 0 ]; then
    python3 -m unittest "$@"
else
    python3 -m unittest discover -v -p "test_*.py"
fi

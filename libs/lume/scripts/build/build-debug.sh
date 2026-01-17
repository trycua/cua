#!/bin/sh

# Get the directory where this script is located
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# Navigate to the lume root directory (two levels up from scripts/build/)
LUME_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

cd "$LUME_DIR"

swift build --product lume
codesign --force --entitlement resources/lume.entitlements --sign - .build/debug/lume

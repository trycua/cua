#!/usr/bin/env bash
# Run cua-driver-rs integration tests.
#
# Usage:
#   ./run_tests.sh                        # run all integration tests
#   ./run_tests.sh test_list_windows      # run a specific test module
#   ./run_tests.sh test_cli test_list_windows  # run multiple modules
#
# The script auto-builds cua-driver (debug) before running unless
# CUA_DRIVER_BINARY is already set in the environment.
#
# Options:
#   -v / --verbose  Pass -v to unittest for verbose output.
#   --release       Build with --release (sets CUA_DRIVER_BINARY to release binary).
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

VERBOSE=""
PROFILE="debug"
MODULES=()

for arg in "$@"; do
    case "$arg" in
        -v|--verbose) VERBOSE="-v" ;;
        --release)    PROFILE="release" ;;
        test_*)       MODULES+=("$arg") ;;
        *)            echo "Unknown argument: $arg"; exit 1 ;;
    esac
done

# Build the binary if CUA_DRIVER_BINARY is not already set.
if [ -z "${CUA_DRIVER_BINARY:-}" ]; then
    echo "==> Building cua-driver ($PROFILE)..."
    CARGO_FLAGS=""
    [ "$PROFILE" = "release" ] && CARGO_FLAGS="--release"
    (cd "$WORKSPACE_ROOT" && cargo build $CARGO_FLAGS -p cua-driver 2>&1)
    export CUA_DRIVER_BINARY="$WORKSPACE_ROOT/target/$PROFILE/cua-driver"
fi

echo "==> Using binary: $CUA_DRIVER_BINARY"
if [ ! -x "$CUA_DRIVER_BINARY" ]; then
    echo "ERROR: binary not found or not executable: $CUA_DRIVER_BINARY"
    exit 1
fi

cd "$SCRIPT_DIR"

if [ "${#MODULES[@]}" -eq 0 ]; then
    # Discover all test_*.py files.
    mapfile -t MODULES < <(find . -maxdepth 1 -name 'test_*.py' -exec basename {} .py \; | sort)
fi

echo "==> Running ${#MODULES[@]} test module(s): ${MODULES[*]}"
echo ""

python3 -m unittest ${VERBOSE} "${MODULES[@]}"

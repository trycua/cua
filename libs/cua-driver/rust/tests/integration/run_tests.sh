#!/usr/bin/env bash
# Run cua-driver-rs integration tests.
#
# Usage:
#   ./run_tests.sh                        # run all tests (Rust binary)
#   ./run_tests.sh test_list_windows      # run a specific test module
#   ./run_tests.sh test_cli test_api_parity  # run multiple modules
#   ./run_tests.sh --parity               # run parity suite against BOTH binaries
#
# The script auto-builds cua-driver (debug) before running unless
# CUA_DRIVER_BINARY is already set in the environment.
#
# Options:
#   -v / --verbose  Pass -v to unittest for verbose output.
#   --release       Build with --release (sets CUA_DRIVER_BINARY to release binary).
#   --parity        Run test_api_parity.py against both Swift and Rust binaries.
#                   Requires CUA_SWIFT_BINARY or ~/.local/bin/cua-driver.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

VERBOSE=""
PROFILE="debug"
PARITY_MODE=0
MODULES=()

for arg in "$@"; do
    case "$arg" in
        -v|--verbose) VERBOSE="-v" ;;
        --release)    PROFILE="release" ;;
        --parity)     PARITY_MODE=1 ;;
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

echo "==> Using Rust binary: $CUA_DRIVER_BINARY"
if [ ! -x "$CUA_DRIVER_BINARY" ]; then
    echo "ERROR: binary not found or not executable: $CUA_DRIVER_BINARY"
    exit 1
fi

# Auto-detect Swift binary for parity mode.
if [ "$PARITY_MODE" -eq 1 ] && [ -z "${CUA_SWIFT_BINARY:-}" ]; then
    for candidate in "$HOME/.local/bin/cua-driver" "$(which cua-driver 2>/dev/null || true)"; do
        if [ -n "$candidate" ] && [ -x "$candidate" ]; then
            export CUA_SWIFT_BINARY="$candidate"
            break
        fi
    done
    if [ -z "${CUA_SWIFT_BINARY:-}" ]; then
        echo "WARNING: --parity requested but no Swift binary found."
        echo "  Install cua-driver or set CUA_SWIFT_BINARY."
        echo "  SwiftParityTests will be skipped."
    else
        echo "==> Using Swift binary: $CUA_SWIFT_BINARY"
    fi
fi

cd "$SCRIPT_DIR"

if [ "$PARITY_MODE" -eq 1 ]; then
    echo "==> Running parity suite against both binaries..."
    python3 -m unittest ${VERBOSE} test_api_parity
    exit $?
fi

if [ "${#MODULES[@]}" -eq 0 ]; then
    # Discover all test_*.py files.
    mapfile -t MODULES < <(find . -maxdepth 1 -name 'test_*.py' -exec basename {} .py \; | sort)
fi

echo "==> Running ${#MODULES[@]} test module(s): ${MODULES[*]}"
echo ""

python3 -m unittest ${VERBOSE} "${MODULES[@]}"

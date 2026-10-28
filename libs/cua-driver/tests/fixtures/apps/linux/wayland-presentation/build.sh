#!/usr/bin/env bash
# Stage the Wayland presentation-timestamp latency fixture for cua-driver tests.
#
# The fixture is a raw Wayland client rather than a toolkit app on purpose: it
# owns its own wl_surface, so wp_presentation.feedback can be requested for
# each fixture-owned content commit.
#
# Build output is copied into
# libs/cua-driver/rust/test-apps/harness-wayland-presentation/ with the
# deterministic executable name the Rust tests expect.
set -euo pipefail

fixtureDir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
linuxDir="$(dirname "$fixtureDir")"
appsDir="$(dirname "$linuxDir")"
harnessDir="$(dirname "$appsDir")"
cuaDriverDir="$(cd "$harnessDir/../.." && pwd)"
outDir="$cuaDriverDir/rust/test-apps/harness-wayland-presentation"
targetDir="${CARGO_TARGET_DIR:-$fixtureDir/target}"

if [ "$(uname -s)" != "Linux" ]; then
  echo "[ERROR] The Wayland presentation fixture only builds on Linux." >&2
  exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "[ERROR] cargo not on PATH. Install Rust first." >&2
  exit 1
fi

# No system Wayland library is required: wayland-client's default backend is
# the pure-Rust protocol implementation, so this builds with cargo alone.
echo "[BUILD] cargo build --release (wayland presentation fixture)"
CARGO_TARGET_DIR="$targetDir" \
  cargo build --release --manifest-path "$fixtureDir/Cargo.toml"

srcBin="$targetDir/release/cua-harness-wayland-presentation"
[ -x "$srcBin" ] || { echo "[ERROR] fixture binary missing after build" >&2; exit 1; }

rm -rf "$outDir"
mkdir -p "$outDir"
cp "$srcBin" "$outDir/CuaTestHarness.WaylandPresentation"
chmod +x "$outDir/CuaTestHarness.WaylandPresentation"
echo "[OK]    Staged: $outDir/CuaTestHarness.WaylandPresentation"

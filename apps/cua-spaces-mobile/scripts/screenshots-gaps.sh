#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
#
# Real screenshots of the new screens (start a task, tool rows, approvals, Queue, offline, diffs, voice, Settings) on
# the sample-data build, iPhone and iPad, light and dark:
#   scripts/screenshots-gaps.sh <CuaSpaces.app> <out dir>
# Drives the app with Maestro (e2e/gaps/*.yaml) on the simulators `cua-mobile-iphone` and `cua-mobile-ipad`, never the
# owner's. Needs Java 17 for Maestro.
set -euo pipefail
APP="$1"; OUT="$2"
HERE="$(cd "$(dirname "$0")/.." && pwd)"
BUNDLE=ai.cua.spaces.mobile
mkdir -p "$OUT"
run() { # udid name flow
  local tmp; tmp="$(mktemp -d)"
  maestro --device "$1" test --test-output-dir "$tmp" -e NAME="$2" "$HERE/e2e/gaps/$3.yaml"
  find "$tmp" -path '*takeScreenshot*' -name '*.png' -exec mv {} "$OUT/" \;
  rm -rf "$tmp"
}
for dev in ${DEVICES:-iphone ipad}; do
  udid="$(xcrun simctl list devices | awk -F'[()]' "/cua-mobile-$dev/{print \$2; exit}")"
  xcrun simctl boot "$udid" 2>/dev/null || true
  xcrun simctl bootstatus "$udid" -b >/dev/null
  xcrun simctl status_bar "$udid" override --time "10:45" --batteryState charged --batteryLevel 100 --cellularMode active --cellularBars 4 --wifiBars 3 >/dev/null
  xcrun simctl install "$udid" "$APP"
  xcrun simctl ui "$udid" content_size large
  flow="phone"; [ "$dev" = "ipad" ] && flow="pad"
  for scheme in ${SCHEMES-light dark}; do
    xcrun simctl ui "$udid" appearance "$scheme"
    run "$udid" "$dev-$scheme" "$flow"
  done
  # Dictation: the permission states are the system's. Refused first, then allowed (the real recognizer hears the Mac's
  # microphone, and a silent press ends with the core's "Didn't hear anything").
  xcrun simctl ui "$udid" appearance light
  # The refused-permission state cannot be set from simctl (revoke is not permitted for speech); it is covered by jest and the core golden.
  xcrun simctl privacy "$udid" grant microphone "$BUNDLE" || true
  xcrun simctl privacy "$udid" grant speech-recognition "$BUNDLE" || true
  run "$udid" "$dev-light" voice-silent
  xcrun simctl privacy "$udid" reset all "$BUNDLE" || true
  xcrun simctl status_bar "$udid" clear >/dev/null || true
done

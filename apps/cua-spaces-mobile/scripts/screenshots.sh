#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
#
# Real screenshots of the sample-data build on the iOS Simulator, light and dark, iPhone and iPad:
#   scripts/screenshots.sh <CuaSpaces.app> <out dir>
# Drives the app with Maestro (e2e/screenshots/*.yaml). Uses two simulators created for this
# (cua-mobile-iphone, cua-mobile-ipad), never the owner's. Needs Java 17 for Maestro.
set -euo pipefail
APP="$1"; OUT="$2"
HERE="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$OUT"
# One Maestro run; its screenshots are collected from its output folder into $OUT.
run() { # udid name flow
  local tmp; tmp="$(mktemp -d)"
  maestro --device "$1" test --test-output-dir "$tmp" -e NAME="$2" "$HERE/e2e/screenshots/$3.yaml"
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
  if [ "$dev" = "iphone" ]; then
    # Dynamic Type at the extra-large accessibility size (iPhone).
    xcrun simctl ui "$udid" content_size accessibility-extra-large
    xcrun simctl ui "$udid" appearance light
    run "$udid" "$dev-xl-text" "phone-xl"
    xcrun simctl ui "$udid" content_size large
  fi
  xcrun simctl status_bar "$udid" clear >/dev/null || true
done

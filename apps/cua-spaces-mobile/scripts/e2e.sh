#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
# Runs e2e/smoke.yaml (Maestro) on the cua-mobile-iphone simulator. The app must be the
# sample-data build: EXPO_PUBLIC_CUA_SAMPLE=1 xcodebuild ... (see README). Needs Java 17 for Maestro.
set -euo pipefail
APP="$1"; HERE="$(cd "$(dirname "$0")/.." && pwd)"
UDID="$(xcrun simctl list devices | awk -F'[()]' '/cua-mobile-iphone/{print $2; exit}')"
xcrun simctl boot "$UDID" 2>/dev/null || true
xcrun simctl bootstatus "$UDID" -b >/dev/null
xcrun simctl install "$UDID" "$APP"
maestro --device "$UDID" test "$HERE/e2e/smoke.yaml"

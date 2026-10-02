#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Builds and signs "Cua Spacesd.app" under $CARGO_TARGET_DIR/macos.
#
# Env overrides:
#   CUA_ENV_CODESIGN_IDENTITY   signing identity (default "-", ad-hoc)
#   CUA_ENV_BUNDLE_ID           CFBundleIdentifier (default com.trycua.cua-env-driver)
#   CUA_ENV_BUNDLE_VERSION      CFBundleShortVersionString (default: Info.plist's)
#   CUA_SPACESD_PREBUILT     package this binary (e.g. a lipo'd universal
#                               build) instead of running cargo build
#   CUA_ENV_CODESIGN_HARDENED=1 sign with the hardened runtime and a secure
#                               timestamp, as notarization requires
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
APP="$TARGET_DIR/macos/Cua Spacesd.app"
EXECUTABLE="$APP/Contents/MacOS/cua-spacesd"
SIGN_IDENTITY="${CUA_ENV_CODESIGN_IDENTITY:--}"
BUNDLE_ID="${CUA_ENV_BUNDLE_ID:-com.trycua.cua-env-driver}"

PREBUILT="${CUA_SPACESD_PREBUILT:-${CUA_GUESTD_PREBUILT:-${CUA_ENV_DRIVER_PREBUILT:-}}}"
SIGN_FLAGS=(--force --deep)
if [ "${CUA_ENV_CODESIGN_HARDENED:-0}" = 1 ]; then
    SIGN_FLAGS+=(--options runtime --timestamp)
fi

cd "$ROOT"
if [ -n "$PREBUILT" ]; then
    [ -f "$PREBUILT" ] || { echo "CUA_SPACESD_PREBUILT=$PREBUILT does not exist" >&2; exit 1; }
else
    cargo build -p cua-spacesd --release
    PREBUILT="$TARGET_DIR/release/cua-spacesd"
fi

mkdir -p "$APP/Contents/MacOS"
install -m 755 "$PREBUILT" "$EXECUTABLE"
install -m 644 packaging/macos/Info.plist "$APP/Contents/Info.plist"
plutil -replace CFBundleIdentifier -string "$BUNDLE_ID" "$APP/Contents/Info.plist"
if [ -n "${CUA_ENV_BUNDLE_VERSION:-}" ]; then
    plutil -replace CFBundleShortVersionString -string "$CUA_ENV_BUNDLE_VERSION" "$APP/Contents/Info.plist"
fi

if ! otool -l "$EXECUTABLE" | grep -A2 LC_RPATH | grep -q '/usr/lib/swift'; then
    install_name_tool -add_rpath /usr/lib/swift "$EXECUTABLE"
fi

codesign "${SIGN_FLAGS[@]}" --sign "$SIGN_IDENTITY" "$APP"
codesign --verify --deep --strict "$APP"

echo "$APP"
if [ "$SIGN_IDENTITY" = "-" ]; then
    echo "warning: ad-hoc signing changes identity after rebuild; set CUA_ENV_CODESIGN_IDENTITY to a stable local or Developer ID signing identity" >&2
fi

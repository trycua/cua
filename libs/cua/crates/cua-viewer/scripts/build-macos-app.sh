#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Builds and signs "Cua Viewer.app" from the libs/cua workspace.
set -euo pipefail

CRATE="$(cd "$(dirname "$0")/.." && pwd)"
WORKSPACE="$(cd "$CRATE/../.." && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-$WORKSPACE/target}"
APP="$TARGET_DIR/macos/Cua Viewer.app"
EXECUTABLE="$APP/Contents/MacOS/cua-viewer"
SIGN_IDENTITY="${CUA_ENV_CODESIGN_IDENTITY:--}"
BUNDLE_ID="${CUA_ENV_CLIENT_BUNDLE_ID:-com.trycua.cua-viewer}"

cd "$WORKSPACE"
cargo build -p cua-viewer --release

mkdir -p "$APP/Contents/MacOS"
install -m 755 "$TARGET_DIR/release/cua-viewer" "$EXECUTABLE"
install -m 644 "$CRATE/packaging/macos/Info.plist" "$APP/Contents/Info.plist"
plutil -replace CFBundleIdentifier -string "$BUNDLE_ID" "$APP/Contents/Info.plist"

codesign --force --deep --sign "$SIGN_IDENTITY" "$APP"
codesign --verify --deep --strict "$APP"

echo "$APP"
if [ "$SIGN_IDENTITY" = "-" ]; then
    echo "warning: use CUA_ENV_CODESIGN_IDENTITY for a stable local or Developer ID identity" >&2
fi

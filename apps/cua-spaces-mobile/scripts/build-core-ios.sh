#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
#
# Builds the Rust app core (core/) as a static xcframework for the iOS
# Simulator and device, into modules/cua-core/ios/CuaCoreLib.xcframework.
# Needs: rustup target add aarch64-apple-ios aarch64-apple-ios-sim
#        --toolchain 1.97.1
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_INCREMENTAL=0
TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/core/target}"
export CARGO_TARGET_DIR="$TARGET_DIR"
(cd core && cargo build --release --target aarch64-apple-ios-sim --target aarch64-apple-ios)
OUT=modules/cua-core/ios/CuaCoreLib.xcframework
HDR=$(mktemp -d)
cp core/include/cua_core.h "$HDR/"
cat > "$HDR/module.modulemap" <<'MM'
module CuaCoreLib { header "cua_core.h" export * }
MM
rm -rf "$OUT"
xcodebuild -create-xcframework \
  -library "$TARGET_DIR/aarch64-apple-ios/release/libcua_spaces_mobile_core.a" -headers "$HDR" \
  -library "$TARGET_DIR/aarch64-apple-ios-sim/release/libcua_spaces_mobile_core.a" -headers "$HDR" \
  -output "$OUT"
echo "built $OUT"

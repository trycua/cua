#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Copy Cua Driver's default cursor theme (the real .lottie the driver's own
# overlay is built from) into the app's resources. `--check` exits 1 when the
# copy has drifted from the source.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
src=../../libs/cua-driver/rust/crates/cursor-overlay/assets/cua.default.lottie
dst=Sources/InfiniteCanvasApp/Resources/cua.default.lottie
if [[ "${1:-}" == --check ]]; then
  cmp -s "$src" "$dst" || { echo "$dst is out of date: run scripts/sync-cursor-assets.sh" >&2; exit 1; }
  exit 0
fi
cp "$src" "$dst"

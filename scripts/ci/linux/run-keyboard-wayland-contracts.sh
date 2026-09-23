#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
: "${SWAYSOCK:?Sway IPC is required}"
: "${WAYLAND_DISPLAY:?A native Wayland display is required}"
export GDK_BACKEND=wayland
mkdir -p "${REPO_ROOT}/artifacts/cua-driver/linux"
cargo test --manifest-path "${REPO_ROOT}/libs/cua-driver/rust/Cargo.toml" \
  -p platform-linux --lib keyboard_wayland_producer_tests --locked \
  -- --ignored --nocapture --test-threads=1 2>&1 \
  | tee "${REPO_ROOT}/artifacts/cua-driver/linux/keyboard-wayland-producer.log"

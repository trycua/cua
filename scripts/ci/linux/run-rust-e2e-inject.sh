#!/usr/bin/env bash
# Run the canonical Rust matrix inside the nested cua-compositor environment.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export CUA_E2E_WAYLAND_SESSION=cua-compositor
exec "${SCRIPT_DIR}/run-rust-e2e-wayland.sh" "$@"

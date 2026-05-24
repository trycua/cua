#!/usr/bin/env bash
# cua-driver local/debug installer — multi-backend dispatcher.
#
# Mirrors the shape of install.sh: a single user-visible entry point at
# libs/cua-driver/scripts/install-local.sh that delegates to a per-backend
# private helper based on host + flags.
#
# Helpers (private — do not invoke directly):
#   _install-local-swift.sh   Builds CuaDriver.app from libs/cua-driver/swift
#                             and installs to /Applications + ~/.local/bin
#   _install-local-rust.sh    Builds cua-driver from libs/cua-driver/rust
#                             and installs the cross-platform binary
#
# Defaults:
#   macOS   → Swift backend (today's default; pass --backend=rust to override)
#   non-mac → Rust backend (Swift is macOS-only; auto-selected)
#
# Flags (forwarded verbatim to whichever helper runs):
#   --experimental-rust    opt into the Rust port (same as --backend=rust)
#   --backend=swift|rust   explicit backend selector
#   --release              build the release configuration (default: debug)
#   --daemon | --autostart pass through to the backend helper
#   --bin-dir <path>       override the symlink destination (default ~/.local/bin)
#
# Not for end-users — see install.sh for the curl-pipe-bash one-liner that
# fetches the signed release tarball.
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

# --- Lightweight flag parsing (mirror install.sh) -----------------------
# Two-pass shape: collect every unrecognised arg into FORWARDED_ARGS so the
# selected backend helper sees the same argv shape as before the wrapper
# existed. Recognised dispatch flags (--experimental-rust, --backend=*) are
# consumed here and never forwarded.
USE_RUST_BACKEND=0
FORWARDED_ARGS=()
PASSTHROUGH=0
while [[ $# -gt 0 ]]; do
    if [[ "$PASSTHROUGH" == "1" ]]; then
        FORWARDED_ARGS+=("$1"); shift; continue
    fi
    case "$1" in
        --experimental-rust) USE_RUST_BACKEND=1; shift ;;
        --backend=rust)      USE_RUST_BACKEND=1; shift ;;
        --backend=swift)     shift ;;                 # explicit default — no-op
        --backend=*)
            printf 'error: unknown backend %q; supported: swift, rust\n' "${1#*=}" >&2
            exit 2
            ;;
        --)                  PASSTHROUGH=1; shift ;;  # forward the rest verbatim
        *)                   FORWARDED_ARGS+=("$1"); shift ;;
    esac
done

# Auto-select Rust on non-macOS — Swift has no install path off Darwin.
if [[ "$USE_RUST_BACKEND" == "0" && "$(uname -s 2>/dev/null)" != "Darwin" ]]; then
    USE_RUST_BACKEND=1
    printf 'note: detected non-macOS host (%s); auto-selecting the cua-driver-rs Rust backend.\n' \
        "$(uname -s 2>/dev/null || echo unknown)" >&2
fi

if [[ "$USE_RUST_BACKEND" == "1" ]]; then
    HELPER="$SCRIPT_DIR/_install-local-rust.sh"
else
    HELPER="$SCRIPT_DIR/_install-local-swift.sh"
fi

if [[ ! -f "$HELPER" ]]; then
    printf 'error: backend helper not found at %s\n' "$HELPER" >&2
    exit 1
fi

# `${arr[@]+"${arr[@]}"}` guards `set -u` on the zero-arg case (macOS bash 3.2).
exec /bin/bash "$HELPER" ${FORWARDED_ARGS[@]+"${FORWARDED_ARGS[@]}"}

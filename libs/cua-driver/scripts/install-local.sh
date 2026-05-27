#!/usr/bin/env bash
# cua-driver local/debug installer — multi-backend dispatcher.
#
# Mirrors the shape of install.sh: a single user-visible entry point at
# libs/cua-driver/scripts/install-local.sh that delegates to a per-backend
# private helper based on host + flags.
#
# Helpers (private — do not invoke directly):
#   _install-local-rust.sh    Builds cua-driver from libs/cua-driver/rust
#                             and installs the cross-platform binary
#   _install-local-swift.sh   Builds CuaDriver.app from libs/cua-driver/swift
#                             and installs to /Applications + ~/.local/bin
#
# Defaults (this script only — install.sh keeps the Swift default on macOS):
#   all hosts → Rust backend
#
# Why the local installer defaults to Rust while the production install.sh
# still gates it behind --experimental-rust: this script is for developers
# working on the checked-out tree, where the Rust port is the active
# development target across all three platforms (Windows / Linux / macOS)
# and has the full integration-test surface attached (see
# crates/cua-driver/tests/harness_*_test.rs). The production curl-pipe-bash
# stays on Swift on macOS until the Rust port flips experimental → stable.
#
# Flags (forwarded verbatim to whichever helper runs):
#   --backend=swift|rust   explicit backend selector (defaults to rust here)
#   --experimental-rust    legacy alias for --backend=rust; now a no-op
#                          since rust is the default. Accepted for
#                          backward compat with scripts/docs that still
#                          pass it.
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
# existed. Recognised dispatch flags (--backend=*, --experimental-rust)
# are consumed here and never forwarded.
USE_SWIFT_BACKEND=0
FORWARDED_ARGS=()
PASSTHROUGH=0
while [[ $# -gt 0 ]]; do
    if [[ "$PASSTHROUGH" == "1" ]]; then
        FORWARDED_ARGS+=("$1"); shift; continue
    fi
    case "$1" in
        --backend=swift)     USE_SWIFT_BACKEND=1; shift ;;
        --backend=rust)      shift ;;                 # explicit default — no-op
        --experimental-rust) shift ;;                 # legacy alias for --backend=rust
        --backend=*)
            printf 'error: unknown backend %q; supported: swift, rust\n' "${1#*=}" >&2
            exit 2
            ;;
        --)                  PASSTHROUGH=1; shift ;;  # forward the rest verbatim
        *)                   FORWARDED_ARGS+=("$1"); shift ;;
    esac
done

# Hard-error on --backend=swift off-Darwin — there's no Swift install path
# anywhere else and the helper would just bail with a confusing error.
if [[ "$USE_SWIFT_BACKEND" == "1" && "$(uname -s 2>/dev/null)" != "Darwin" ]]; then
    printf 'error: --backend=swift requested on non-macOS host (%s); Swift binary is macOS-only.\n' \
        "$(uname -s 2>/dev/null || echo unknown)" >&2
    exit 2
fi

if [[ "$USE_SWIFT_BACKEND" == "1" ]]; then
    HELPER="$SCRIPT_DIR/_install-local-swift.sh"
else
    HELPER="$SCRIPT_DIR/_install-local-rust.sh"
fi

if [[ ! -f "$HELPER" ]]; then
    printf 'error: backend helper not found at %s\n' "$HELPER" >&2
    exit 1
fi

# `${arr[@]+"${arr[@]}"}` guards `set -u` on the zero-arg case (macOS bash 3.2).
exec /bin/bash "$HELPER" ${FORWARDED_ARGS[@]+"${FORWARDED_ARGS[@]}"}

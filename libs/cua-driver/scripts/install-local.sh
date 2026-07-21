#!/usr/bin/env bash
# cua-driver local/debug installer — Rust backend only.
#
# This script is for developers working on the checked-out tree. It builds
# cua-driver from libs/cua-driver/rust and installs it as `cua-driver-local`,
# separate from any released `cua-driver` installation.
#
# Flags (forwarded verbatim to the Rust helper):
#   --release              build the release configuration (default: debug)
#   --autostart            register an auto-start daemon (macOS: LaunchAgent;
#                          Linux: systemd user unit). Default off.
#   --bin-dir <path>       override the symlink destination (default ~/.local/bin)
#   --backend=rust         explicit Rust backend (no-op; Rust is the only option)
#   --experimental-rust    legacy alias for --backend=rust (no-op)
#   --backend=swift        retired Swift backend (no-op; accepted for compat)
#
# Not for end-users — see install.sh for the curl-pipe-bash one-liner that
# fetches the signed release tarball.
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

# --- Lightweight flag parsing -----------------------------------------------
# Consume backend flags (no-ops for compat) and forward the rest to the Rust helper.
FORWARDED_ARGS=()
PASSTHROUGH=0
while [[ $# -gt 0 ]]; do
    if [[ "$PASSTHROUGH" == "1" ]]; then
        FORWARDED_ARGS+=("$1"); shift; continue
    fi
    case "$1" in
        --backend=rust)      shift ;;                 # explicit Rust (no-op)
        --experimental-rust) shift ;;                 # legacy alias (no-op)
        --backend=swift)     shift ;;                 # retired Swift (no-op)
        --backend=*)
            printf 'error: unknown backend %q; supported: rust\n' "${1#*=}" >&2
            exit 2
            ;;
        --)                  PASSTHROUGH=1; shift ;;  # forward the rest verbatim
        *)                   FORWARDED_ARGS+=("$1"); shift ;;
    esac
done

HELPER="$SCRIPT_DIR/_install-local-rust.sh"
if [[ ! -f "$HELPER" ]]; then
    printf 'error: backend helper not found at %s\n' "$HELPER" >&2
    exit 1
fi

# `${arr[@]+"${arr[@]}"}` guards `set -u` on the zero-arg case (macOS bash 3.2).
exec /bin/bash "$HELPER" ${FORWARDED_ARGS[@]+"${FORWARDED_ARGS[@]}"}

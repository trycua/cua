#!/bin/bash
# cua-driver installer — download and install the Rust implementation.
# Downloads the latest release from GitHub Releases and wires it into the user's PATH.
#
# Usage (from README + release body):
#   /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver/scripts/install.sh)"
#
# Flags:
#   --bin-dir <path>     install the cua-driver wrapper to <path> instead of
#                        ~/.local/bin (e.g. /usr/local/bin — that target needs sudo)
#   --no-modify-path     skip auto-appending an `export PATH=...` line to your
#                        shell rc when ~/.local/bin is missing from PATH
#   --backend=rust       explicit Rust backend (no-op; Rust is the only option)
#   --experimental-rust  legacy alias for --backend=rust (no-op)
#   --backend=swift      retired Swift backend (no-op; accepted for compat)
#
# Env overrides:
#   CUA_DRIVER_RS_VERSION=0.6.8    pin a specific Rust release tag
#   CUA_DRIVER_VERSION=0.6.8       legacy alias for CUA_DRIVER_RS_VERSION
#   CUA_DRIVER_RS_INSTALL_DIR=PATH same as --bin-dir
#   CUA_DRIVER_BIN_DIR=PATH        legacy alias for --bin-dir
#   CUA_DRIVER_NO_MODIFY_PATH=1    same as --no-modify-path
#
# Uninstall:
#   /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver/scripts/uninstall.sh)"
set -euo pipefail

BIN_DIR="${CUA_DRIVER_BIN_DIR:-$HOME/.local/bin}"
NO_MODIFY_PATH="${CUA_DRIVER_NO_MODIFY_PATH:-0}"

# Rust implementation delegation target. The Rust install logic is a private
# helper script colocated with this one — _install-rust.sh — so that
# this directory holds the single user-facing install.sh per platform.
# The Rust path below either execs the on-disk helper (dev /
# checked-out-tree case) or curls this URL and pipes it to bash.
RUST_INSTALLER_URL="https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver/scripts/_install-rust.sh"

# Lightweight flag parsing (avoid getopt; macOS getopt is GNU-incompatible).
#
# Two-pass shape:
#   1. Walk all argv and collect every unrecognised arg into FORWARDED_ARGS.
#      That bucket is what we'd hand off to the Rust installer. Recognised
#      shared flags (--bin-dir, --no-modify-path) are consumed in this pass
#      and applied to local state.
#   2. Always exec the Rust installer with FORWARDED_ARGS. Backend flags
#      (--backend=rust/swift, --experimental-rust) are no-ops for compat.
#
# This lets backend flags appear at any position, lets `--` end flag
# parsing without breaking forwarding, and keeps the Rust installer's argv
# shape bit-compatible.
FORWARDED_ARGS=()
PASSTHROUGH=0
while [[ $# -gt 0 ]]; do
    if [[ "$PASSTHROUGH" == "1" ]]; then
        FORWARDED_ARGS+=("$1"); shift; continue
    fi
    case "$1" in
        --experimental-rust) shift ;;  # legacy alias (no-op)
        --backend=rust)      shift ;;  # explicit Rust (no-op)
        --backend=swift)     shift ;;  # retired Swift (no-op)
        --backend=*)
            printf 'error: unknown backend %q; supported: rust\n' "${1#*=}" >&2
            exit 2
            ;;
        --bin-dir)
            if [[ -z "${2:-}" || "${2:0:1}" == "-" ]]; then
                printf 'error: --bin-dir requires a value\n' >&2
                exit 2
            fi
            BIN_DIR="$2"; FORWARDED_ARGS+=("$1" "$2"); shift 2 ;;
        --bin-dir=*)         BIN_DIR="${1#*=}"; FORWARDED_ARGS+=("$1"); shift ;;
        --no-modify-path)    NO_MODIFY_PATH=1; FORWARDED_ARGS+=("$1"); shift ;;
        --)                  PASSTHROUGH=1; shift ;;  # forward the rest verbatim
        *)                   FORWARDED_ARGS+=("$1"); shift ;;
    esac
done

# --- Delegate to the Rust installer ----------------------------------------
printf 'note: installing cua-driver via the Rust implementation.\n' >&2

if [[ -n "${CUA_DRIVER_BIN_DIR:-}" && -z "${CUA_DRIVER_RS_INSTALL_DIR:-}" ]]; then
    export CUA_DRIVER_RS_INSTALL_DIR="$BIN_DIR"
fi
if [[ -n "${CUA_DRIVER_VERSION:-}" && -z "${CUA_DRIVER_RS_VERSION:-}" ]]; then
    export CUA_DRIVER_RS_VERSION="$CUA_DRIVER_VERSION"
fi
if [[ -n "${CUA_DRIVER_NO_MODIFY_PATH:-}" && -z "${CUA_DRIVER_RS_NO_MODIFY_PATH:-}" ]]; then
    export CUA_DRIVER_RS_NO_MODIFY_PATH="$CUA_DRIVER_NO_MODIFY_PATH"
fi

# Prefer the on-disk copy when this script is running from a checked-out
# tree (dev / CI). Falls back to curling the canonical URL for the
# `curl ... | bash` install path, where $BASH_SOURCE is unset / -.
LOCAL_RUST_INSTALLER=""
if [[ -n "${BASH_SOURCE[0]:-}" && "${BASH_SOURCE[0]}" != "-" && -f "${BASH_SOURCE[0]}" ]]; then
    SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
    # Helper lives next to this script under libs/cua-driver/scripts/.
    CANDIDATE="$SCRIPT_DIR/_install-rust.sh"
    if [[ -f "$CANDIDATE" ]]; then
        LOCAL_RUST_INSTALLER="$CANDIDATE"
    fi
fi

# macOS ships bash 3.2, which trips `set -u` when expanding an empty
# array via "${arr[@]}" — guard with the +alt-value pattern so the
# zero-arg case becomes a literal no-expansion.
if [[ -n "$LOCAL_RUST_INSTALLER" ]]; then
    exec /bin/bash "$LOCAL_RUST_INSTALLER" ${FORWARDED_ARGS[@]+"${FORWARDED_ARGS[@]}"}
else
    if ! command -v curl >/dev/null 2>&1; then
        printf 'error: curl not found on PATH; cannot fetch %s\n' "$RUST_INSTALLER_URL" >&2
        exit 1
    fi
    # `exec` so the Rust installer replaces this process.
    RUST_INSTALLER_SCRIPT="$(curl -fsSL "$RUST_INSTALLER_URL")" || {
        printf 'error: failed to download Rust installer from %s\n' "$RUST_INSTALLER_URL" >&2
        exit 1
    }
    exec /bin/bash -c "$RUST_INSTALLER_SCRIPT" cua-driver-rs-install ${FORWARDED_ARGS[@]+"${FORWARDED_ARGS[@]}"}
fi

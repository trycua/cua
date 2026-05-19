#!/usr/bin/env bash
#
# cua-driver-rs/scripts/install.sh — backward-compat redirect.
#
# The canonical user-facing installer now lives at
# libs/cua-driver/scripts/install.sh (Swift driver's installer that
# auto-delegates to the Rust port on non-macOS hosts or when
# `--experimental-rust` is passed). This shim exists so the legacy
# one-liner against this path keeps working forever:
#
#   /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver-rs/scripts/install.sh)"
#
# The actual Rust install logic lives at
# libs/cua-driver/scripts/_install-rust.sh — invoked by the canonical
# script when `--backend=rust` (or auto-detected on Linux). This shim
# just forwards there with `--backend=rust` to skip the auto-detect
# message on Linux and the macOS-Swift-default behavior.

set -euo pipefail

CANONICAL_URL="https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver/scripts/install.sh"

printf 'note: cua-driver-rs/scripts/install.sh is deprecated; fetching the\n' >&2
printf '      canonical installer from cua-driver/scripts/install.sh\n' >&2
printf '      The Rust port is still what gets installed — only the URL moved.\n' >&2
printf '\n' >&2

# Try on-disk first when invoked from a checked-out tree (dev / CI).
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]:-$0}")" && pwd 2>/dev/null || true)"
ON_DISK="$SCRIPT_DIR/../../cua-driver/scripts/install.sh"
if [[ -n "$SCRIPT_DIR" && -f "$ON_DISK" ]]; then
    exec /bin/bash "$ON_DISK" --backend=rust "$@"
fi

if ! command -v curl >/dev/null 2>&1; then
    printf 'error: curl not found on PATH; cannot fetch %s\n' "$CANONICAL_URL" >&2
    exit 1
fi

CANONICAL_SCRIPT="$(curl -fsSL "$CANONICAL_URL")" || {
    printf 'error: failed to download canonical installer from %s\n' "$CANONICAL_URL" >&2
    exit 1
}
exec /bin/bash -c "$CANONICAL_SCRIPT" install --backend=rust "$@"

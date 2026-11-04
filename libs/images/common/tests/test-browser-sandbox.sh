#!/usr/bin/env bash
# Offline test for the browser sandbox choice: common/desktop/browser-sandbox.sh
# (what start-desktop.sh writes into desktop.env) and the Chromium launcher
# snippet that reads it (linux/files/chromium.d/cua-sandbox).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IMAGES="$(cd "$HERE/../.." && pwd)"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
pass=0 fail=0
expect() { # name want got
    if [ "$2" = "$3" ]; then
        pass=$((pass + 1))
    else
        fail=$((fail + 1)); printf 'FAIL %s\n  want: %s\n  got:  %s\n' "$1" "$2" "$3"
    fi
}

# browser_sandbox_env with stubbed probes: GVISOR=0|1 USERNS=0|1 MACHINE=...
env_for() {
    (
        unset CUA_DRIVER_BROWSER_NO_SANDBOX CUA_DRIVER_BROWSER_NO_SECCOMP_SANDBOX
        for kv in "$@"; do export "${kv?}"; done
        # shellcheck source=../desktop/browser-sandbox.sh
        . "$IMAGES/common/desktop/browser-sandbox.sh"
        under_gvisor() { [ "$GVISOR" = 1 ]; }
        unprivileged_userns() { [ "$USERNS" = 1 ]; }
        machine() { echo "$MACHINE"; }
        browser_sandbox_env | grep -v '^MOZ_' | tr '\n' ' '
        echo "moz=$(browser_sandbox_env | grep -c '^MOZ_DISABLE_')"
    )
}

expect "runc with user namespaces: full sandboxes" \
    "moz=0" "$(env_for GVISOR=0 USERNS=1 MACHINE=x86_64)"
expect "runc without user namespaces: --no-sandbox" \
    "CUA_DRIVER_BROWSER_NO_SANDBOX=1 moz=0" "$(env_for GVISOR=0 USERNS=0 MACHINE=aarch64)"
expect "gVisor x86_64: Chromium sandbox kept, Firefox inner sandboxes off" \
    "moz=5" "$(env_for GVISOR=1 USERNS=1 MACHINE=x86_64)"
expect "gVisor arm64: seccomp layer off only" \
    "CUA_DRIVER_BROWSER_NO_SECCOMP_SANDBOX=1 moz=5" "$(env_for GVISOR=1 USERNS=1 MACHINE=aarch64)"
expect "gVisor arm64 never falls back to --no-sandbox" \
    "CUA_DRIVER_BROWSER_NO_SECCOMP_SANDBOX=1 moz=5" "$(env_for GVISOR=1 USERNS=0 MACHINE=aarch64)"
expect "operator override wins over detection" \
    "CUA_DRIVER_BROWSER_NO_SECCOMP_SANDBOX=0 moz=5" \
    "$(env_for GVISOR=1 USERNS=1 MACHINE=aarch64 CUA_DRIVER_BROWSER_NO_SECCOMP_SANDBOX=0)"
expect "operator --no-sandbox is kept" \
    "CUA_DRIVER_BROWSER_NO_SANDBOX=1 moz=0" \
    "$(env_for GVISOR=0 USERNS=1 MACHINE=x86_64 CUA_DRIVER_BROWSER_NO_SANDBOX=1)"

# The launcher snippet, sourced by /bin/sh the way /usr/bin/chromium does.
flags_for() { # desktop.env contents, then optional VAR=value environment
    local envfile="$1"; shift
    mkdir -p "$tmp/run"; printf '%s' "$envfile" >"$tmp/run/desktop.env"
    env -u CUA_DRIVER_BROWSER_NO_SANDBOX -u CUA_DRIVER_BROWSER_NO_SECCOMP_SANDBOX \
        XDG_RUNTIME_DIR="$tmp/run" "$@" sh -c \
        'CHROMIUM_FLAGS=" --base"; . "$0"; echo "$CHROMIUM_FLAGS|${cua_val-unset}"' \
        "$IMAGES/linux/files/chromium.d/cua-sandbox"
}
expect "snippet: nothing in desktop.env" " --base|unset" "$(flags_for 'DISPLAY=:1
')"
expect "snippet: seccomp off" " --base --disable-seccomp-filter-sandbox|unset" \
    "$(flags_for 'DISPLAY=:1
CUA_DRIVER_BROWSER_NO_SECCOMP_SANDBOX=1
')"
expect "snippet: no sandbox" " --base --no-sandbox|unset" \
    "$(flags_for 'CUA_DRIVER_BROWSER_NO_SANDBOX=1
')"
expect "snippet: environment wins over desktop.env" " --base|unset" \
    "$(flags_for 'CUA_DRIVER_BROWSER_NO_SECCOMP_SANDBOX=1
' CUA_DRIVER_BROWSER_NO_SECCOMP_SANDBOX=0)"
expect "snippet: no desktop.env at all" " --base|unset" \
    "$(env -u CUA_DRIVER_BROWSER_NO_SANDBOX -u CUA_DRIVER_BROWSER_NO_SECCOMP_SANDBOX \
        XDG_RUNTIME_DIR="$tmp/missing" sh -c 'CHROMIUM_FLAGS=" --base"; . "$0"; echo "$CHROMIUM_FLAGS|${cua_val-unset}"' \
        "$IMAGES/linux/files/chromium.d/cua-sandbox")"

echo "browser-sandbox: $pass passed, $fail failed"
[ "$fail" = 0 ]

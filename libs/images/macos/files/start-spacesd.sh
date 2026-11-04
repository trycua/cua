#!/bin/bash
# Launch cua-spacesd in the logged-in GUI session (run by the LaunchAgent
# com.trycua.spacesd). It runs from its app bundle so it keeps the bundle's
# TCC identity (Screen Recording, Accessibility).
#
# Token resolution, first match wins (the Linux image's order, macOS paths):
#   1. $CUA_ENV_TOKEN
#   2. the Lume setup share the cua SDK writes at start (cua-vmm):
#      /Volumes/My Shared Files/setup/env-token (lume 0.5 mounts each shared
#      directory under its name) or /Volumes/My Shared Files/env-token
#   3. /etc/cua/env-token                  persistent, written by a provisioner
#   4. ~/.cua/spacesd/token                a token an earlier boot installed
# The winner is written to ~/.cua/spacesd/token (owned by the desktop user,
# 0600, directory 0700), and the driver reads it from there
# (CUA_ENV_TOKEN_FILE). It never goes on argv or into the environment of
# anything the driver starts.
#
# With no token the driver starts in bootstrap mode (--insecure-bootstrap):
# only GetCapabilities, Health and Init answer until the first client (the
# cua SDK) installs a token with SystemService.Init. No token is baked into
# the image.
set -u

APP_BIN="${CUA_SPACESD_BIN:-/Applications/Cua Spacesd.app/Contents/MacOS/cua-spacesd}"
STATE_DIR="$HOME/.cua/spacesd"
TOKEN_FILE="$STATE_DIR/token"
SHARE="/Volumes/My Shared Files"
PORT="${CUA_ENV_PORT:-3211}"

log() { echo "[$(date -u +%Y-%m-%dT%H:%M:%SZ)] start-spacesd: $*"; }

umask 077
mkdir -p "$STATE_DIR"
chmod 700 "$HOME/.cua" "$STATE_DIR" 2>/dev/null

# Writes stdin to the token file atomically, 0600, owned by this user.
install_token() {
    local tmp
    tmp="$(mktemp "$STATE_DIR/.token.XXXXXX")" || return 1
    tr -d '\r\n' >"$tmp"
    [ -s "$tmp" ] || { rm -f "$tmp"; return 1; }
    chmod 600 "$tmp" && mv -f "$tmp" "$TOKEN_FILE"
}

source_desc=""
if [ -n "${CUA_ENV_TOKEN:-}" ]; then
    printf '%s' "$CUA_ENV_TOKEN" | install_token && source_desc="CUA_ENV_TOKEN"
fi
unset CUA_ENV_TOKEN CUA_SPACESD_TOKEN CUA_GUESTD_TOKEN
if [ -z "$source_desc" ]; then
    # The setup share mounts around login; wait for it briefly (bounded).
    for _ in $(seq 1 20); do [ -d "$SHARE" ] && break; sleep 0.5; done
    for f in "$SHARE/setup/env-token" "$SHARE/env-token"; do
        if [ -s "$f" ] && install_token <"$f"; then
            source_desc="setup share"
            break
        fi
    done
    if [ -n "$source_desc" ]; then
        :
    elif [ -r /etc/cua/env-token ] && [ -s /etc/cua/env-token ] && install_token </etc/cua/env-token; then
        source_desc="/etc/cua/env-token"
    elif [ -s "$TOKEN_FILE" ]; then
        chmod 600 "$TOKEN_FILE"
        source_desc="$TOKEN_FILE (kept)"
    fi
fi

export CUA_ENV_TOKEN_FILE="$TOKEN_FILE"
export CUA_ENV_LOG="${CUA_ENV_LOG:-info}"
bootstrap=()
if [ -z "$source_desc" ]; then
    bootstrap=(--insecure-bootstrap)
    source_desc="none, bootstrap mode (Init installs one)"
fi
log "starting $APP_BIN on 0.0.0.0:$PORT (token: $source_desc)"
# bash 3.2 (/bin/bash) treats an empty "${a[@]}" as unbound under set -u.
exec "$APP_BIN" serve --listen "0.0.0.0:$PORT" ${bootstrap[@]+"${bootstrap[@]}"}

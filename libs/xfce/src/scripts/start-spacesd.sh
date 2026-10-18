#!/bin/bash
# Launch cua-spacesd (gRPC + gRPC-Web) on 0.0.0.0:${CUA_ENV_PORT:-3211}.
#
# Token resolution, first match wins:
#   1. $CUA_ENV_TOKEN
#   2. /run/cua/env-token (a mounted secret, cloud-init, or generated below)
# When neither exists a random token is generated into /run/cua/env-token
# (mode 0600) so the driver never listens unauthenticated. Read it with
#   docker exec <container> cat /run/cua/env-token
# The token reaches the driver through the environment, never argv.
set -euo pipefail

BIN="${CUA_SPACESD_BIN:-${CUA_GUESTD_BIN:-${CUA_ENV_DRIVER_BIN:-/usr/local/bin/cua-spacesd}}}"
LISTEN="0.0.0.0:${CUA_ENV_PORT:-3211}"
TOKEN_FILE="${CUA_ENV_TOKEN_FILE:-/run/cua/env-token}"

echo "Waiting for X server to start..."
for _ in $(seq 1 120); do
    xdpyinfo -display "${DISPLAY:-:1}" >/dev/null 2>&1 && break
    sleep 1
done

if [ -z "${CUA_ENV_TOKEN:-}" ]; then
    if [ ! -s "$TOKEN_FILE" ]; then
        install -d -m 0700 "$(dirname "$TOKEN_FILE")"
        (umask 077; head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' >"$TOKEN_FILE")
    fi
    CUA_ENV_TOKEN="$(tr -d '\r\n' <"$TOKEN_FILE")"
fi
export CUA_ENV_TOKEN

export DISPLAY="${DISPLAY:-:1}"
echo "Starting $BIN --listen $LISTEN (DISPLAY=$DISPLAY)"
exec "$BIN" --listen "$LISTEN"

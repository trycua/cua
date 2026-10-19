#!/bin/bash
# Kasm custom startup: launch cua-spacesd on 0.0.0.0:${CUA_ENV_PORT:-3211}.
# Token: $CUA_ENV_TOKEN, else /run/cua/env-token (generated, mode 0600, when
# missing). The token reaches the driver through the environment, never argv.
set -euo pipefail
TOKEN_FILE="${CUA_ENV_TOKEN_FILE:-/run/cua/env-token}"
if [ -z "${CUA_ENV_TOKEN:-}" ]; then
    if [ ! -s "$TOKEN_FILE" ]; then
        sudo install -d -o "$(id -u)" -m 0700 "$(dirname "$TOKEN_FILE")"
        (umask 077; head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' >"$TOKEN_FILE")
    fi
    CUA_ENV_TOKEN="$(tr -d '\r\n' <"$TOKEN_FILE")"
fi
export CUA_ENV_TOKEN
exec /usr/local/bin/cua-spacesd --listen "0.0.0.0:${CUA_ENV_PORT:-3211}"

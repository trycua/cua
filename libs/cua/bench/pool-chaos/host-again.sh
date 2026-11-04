#!/bin/bash
# Starts this machine's host service again (after its cua-spacesd was killed).
cd "$(dirname "$0")"
W=$PWD
# What node-up.sh set this machine up as.
. "$W/node.env"
CUA_ENV_LISTEN="127.0.0.1:$PORT" cua host setup --relay "$CUA_RELAY_URL" --profile spare --name "$NAME" \
  --driver-bin "$CUA_SPACESD_BIN" --runner process --json > "$W/host-again.json" 2>&1
cua auth login --no-browser >/dev/null 2>&1

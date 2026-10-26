#!/bin/bash
# node-up.sh: sign this machine in to the test relay and run its daemon, in an
# isolated home. Env: BIN (dir with cua, cua-spacesd), W (work dir), RELAY,
# ISSUER, NAME, ROLE (provider|client), PORT (host listener, default 3511).
set -euo pipefail
: "${BIN:?}" "${W:?}" "${RELAY:?}" "${ISSUER:?}" "${NAME:?}" "${ROLE:=provider}" "${PORT:=3511}"
mkdir -p "$W/home" "$W/xdg"
# host-again.sh reads what this machine was set up as.
printf 'PORT=%s\nNAME=%q\n' "$PORT" "$NAME" > "$W/node.env"
export CUA_HOME="$W/home" CUA_CREDENTIAL_STORE=file CUA_RELAY_URL="$RELAY" CUA_OIDC_ISSUER="$ISSUER" \
  CUA_TELEMETRY_ENABLED=false CUA_TELEMETRY=off DO_NOT_TRACK=1 CUA_SPACESD_BIN="$BIN/cua-spacesd" \
  PATH="$BIN:$PATH" CUA_DAEMON_AUTOSTART=0
cua daemon stop >/dev/null 2>&1 || true
cua auth login --no-browser 2>&1 | tail -1
cua devices enroll --json >/dev/null 2>&1 || true
if [ "$ROLE" = provider ]; then
  CUA_ENV_LISTEN="127.0.0.1:$PORT" cua host setup --relay "$RELAY" --profile spare --name "$NAME" \
     --driver-bin "$BIN/cua-spacesd" --runner process --json > "$W/host-setup.json" 2>"$W/host-setup.err" || { cat "$W/host-setup.err"; exit 1; }
  # Setup keeps no account session: sign in again so this machine can coordinate.
  cua auth login --no-browser 2>&1 | tail -1
fi
nohup "$BIN/cua" daemon start --foreground > "$W/daemon.log" 2>&1 &
echo $! > "$W/daemon.pid"
for i in $(seq 1 60); do cua daemon status >/dev/null 2>&1 && break; sleep 0.5; done
cua daemon status 2>&1 | head -3

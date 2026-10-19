#!/usr/bin/env bash
# Launch cua-spacesd on CUA_ENV_LISTEN (default 0.0.0.0:3211) inside the
# desktop session.
#
# Token resolution, first match wins:
#   1. $CUA_ENV_TOKEN
#   2. the file named by $CUA_ENV_TOKEN_FILE
#   3. /run/cua/env-token     (written by the container entrypoint hook, a
#                              Fleet/K8s secret mount, or cloud-init)
#   4. /etc/cua/env-token     (persistent, e.g. baked by cloud-init write_files)
# The container entrypoint hook generates /run/cua/env-token when none of the
# above exist, so the driver never starts unauthenticated.
#
# The token reaches the driver through the CUA_ENV_TOKEN environment variable,
# never argv (argv is world-readable in /proc).
#
# Fleet (await-token-file mode, see env-token-mode.sh): no token is resolved
# here. The driver binds with no token, serves only GetCapabilities/Health,
# and follows the claim's token file: /run/cua/env-token directly when this
# user can read it (pod fsGroup + defaultMode 0440), else the copy that the
# root token-sync helper keeps at /run/cua-env/env-token.
#
# Driver interface (libs/cua-spacesd, Linux default mode): started with no
# transport flags it binds 0.0.0.0:$CUA_ENV_PORT and requires $CUA_ENV_TOKEN
# from clients; CUA_ENV_ALLOW_ANONYMOUS=0 makes a missing token fatal instead
# of anonymous. (`--listen` would need `--token` on argv for a non-loopback
# address, so it is not used.) CUA_SPACESD_ARGS appends extra flags.
#
# Relay join (a Space in your cloud, for example a Modal sandbox): with
# CUA_ENV_RELAY_URL set and no CUA_SPACESD_ARGS, the driver runs `join`, which
# reads CUA_RELAY_TOKEN, CUA_ENV_MACHINE_ID, CUA_RELAY_JWKS_JSON and
# CUA_HOST_POLICY_JSON from the environment and dials out to the relay (no
# inbound port).
#
# Every CUA_ENV_* variable can also be spelled CUA_SPACESD_* (new name wins);
# CUA_SPACESD_BIN/_ARGS fall back to the older CUA_GUESTD_* and
# CUA_ENV_DRIVER_* spellings.
set -euo pipefail
. /opt/cua/bin/env-token-mode.sh
BIN="${CUA_SPACESD_BIN:-${CUA_GUESTD_BIN:-${CUA_ENV_DRIVER_BIN:-/usr/local/bin/cua-spacesd}}}"
CUA_SPACESD_ARGS="${CUA_SPACESD_ARGS:-${CUA_GUESTD_ARGS:-${CUA_ENV_DRIVER_ARGS:-}}}"
if [ -z "$CUA_SPACESD_ARGS" ] && [ -n "${CUA_ENV_RELAY_URL:-${CUA_SPACESD_RELAY_URL:-}}" ]; then
    CUA_SPACESD_ARGS=join
fi
export CUA_ENV_PORT="${CUA_ENV_PORT:-3211}"
export CUA_ENV_ALLOW_ANONYMOUS="${CUA_ENV_ALLOW_ANONYMOUS:-0}"

if [ ! -x "$BIN" ]; then
    echo "[$(date -Iseconds)] cua-spacesd not installed at $BIN; idling (image built with CUA_SPACESD_SOURCE=none)"
    exec sleep infinity
fi

if cua_await_token_file; then
    # Follow the claim file directly only when this user can read it AND it
    # is not world-accessible (the driver refuses such files when not root);
    # otherwise use the root token-sync copy.
    if [ -r "$CUA_CLAIM_TOKEN_FILE" ] \
        && [ "$(( 0$(stat -L -c %a "$CUA_CLAIM_TOKEN_FILE" 2>/dev/null || echo 7) & 7 ))" = 0 ]; then
        export CUA_ENV_TOKEN_FILE="$CUA_CLAIM_TOKEN_FILE"
    else
        export CUA_ENV_TOKEN_FILE="$CUA_SYNCED_TOKEN_FILE"
    fi
    export CUA_ENV_AWAIT_TOKEN_FILE=1
    unset CUA_ENV_TOKEN
fi

token="${CUA_ENV_TOKEN:-}"
[ "${CUA_ENV_AWAIT_TOKEN_FILE:-}" = 1 ] && token=await
for f in "${CUA_ENV_TOKEN_FILE:-}" /run/cua/env-token /etc/cua/env-token; do
    [ -n "$token" ] && break
    [ -n "$f" ] && [ -r "$f" ] && token="$(tr -d '\r\n' <"$f")"
done
if [ -z "$token" ]; then
    echo "[$(date -Iseconds)] no spacesd token (CUA_ENV_TOKEN, /run/cua/env-token, /etc/cua/env-token); refusing to start"
    exit 1
fi
if [ "${CUA_ENV_AWAIT_TOKEN_FILE:-}" = 1 ]; then
    token_desc="awaiting token file $CUA_ENV_TOKEN_FILE"
else
    export CUA_ENV_TOKEN="$token"
    token_desc="token set"
fi

ENV_FILE="${CUA_DESKTOP_RUNTIME_DIR:-/run/cua-desktop}/desktop.env"
for _ in $(seq 1 60); do
    [ -r "$ENV_FILE" ] && break
    sleep 1
done
if [ -r "$ENV_FILE" ]; then set -a; . "$ENV_FILE"; set +a; else export DISPLAY="${CUA_DISPLAY:-:1}"; fi

echo "[$(date -Iseconds)] starting $BIN on 0.0.0.0:$CUA_ENV_PORT (DISPLAY=$DISPLAY, $token_desc)"
# shellcheck disable=SC2086  # CUA_SPACESD_ARGS is a flag list
exec "$BIN" ${CUA_SPACESD_ARGS:-}

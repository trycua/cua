#!/usr/bin/env bash
# Materialise the spacesd token at /run/cua/env-token (root:cua 0640).
# Container: run from the entrypoint hook. VM: ExecStartPre of cua-spacesd.service.
# A caller-provided CUA_ENV_TOKEN wins, then /etc/cua/env-token; otherwise a
# random one is generated so the driver never listens unauthenticated.
# Read it back with: cat /run/cua/env-token
#
# Fleet (await-token-file mode): /run/cua is the claim Secret mount; it is
# left alone and no token is generated. See env-token-mode.sh.
set -euo pipefail
. /opt/cua/bin/env-token-mode.sh
if cua_await_token_file; then
    echo "env token: awaiting the claim token file $CUA_CLAIM_TOKEN_FILE"
    exit 0
fi
DESKTOP_USER="${CUA_DESKTOP_USER:-cua}"
install -d -m 0750 -o root -g "$DESKTOP_USER" /run/cua
if [ -n "${CUA_ENV_TOKEN:-}" ]; then
    printf '%s\n' "$CUA_ENV_TOKEN" >/run/cua/env-token
elif [ -s /etc/cua/env-token ]; then
    cp /etc/cua/env-token /run/cua/env-token
elif [ ! -s /run/cua/env-token ]; then
    head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' >/run/cua/env-token
    echo >>/run/cua/env-token
fi
chown root:"$DESKTOP_USER" /run/cua/env-token
chmod 0640 /run/cua/env-token

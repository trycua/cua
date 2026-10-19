#!/usr/bin/env bash
# Privileged half of await-token-file mode (runs as root): mirrors the
# root-only claim token /run/cua/env-token to /run/cua-env/env-token (0600,
# owned by the desktop user) for the unprivileged driver, emptying it on
# release. Idles when this boot is not in await mode or the driver is absent.
set -euo pipefail
. /opt/cua/bin/env-token-mode.sh
BIN="${CUA_SPACESD_BIN:-${CUA_GUESTD_BIN:-${CUA_ENV_DRIVER_BIN:-/usr/local/bin/cua-spacesd}}}"
DESKTOP_USER="${CUA_DESKTOP_USER:-cua}"
if [ ! -x "$BIN" ]; then
    echo "[$(date -Iseconds)] token sync not needed (no cua-spacesd); idling"
    # Named, so a caller that cannot query supervisord (the doctor running
    # as the desktop user) still sees the program in the process table.
    exec -a "cua-env-token-sync (idle)" sleep infinity
fi
if ! cua_await_token_file; then
    # The claim secrets can be mounted after this starts: a local VM's
    # cloud-init mounts /run/cua and writes the token after early boot (and
    # restarts cua-spacesd into await mode), so idling for good would leave
    # the driver waiting on a copy nobody writes. Keep watching; mirroring
    # while the driver uses a local token is harmless (it reads its own file).
    echo "[$(date -Iseconds)] token sync not needed yet (no claim secrets mount or a local token is set); watching $CUA_CLAIM_SECRETS_DIR"
    until cua_is_mountpoint "$CUA_CLAIM_SECRETS_DIR"; do
        (exec -a "cua-env-token-sync (idle)" sleep 2)
    done
    echo "[$(date -Iseconds)] claim secrets mounted at $CUA_CLAIM_SECRETS_DIR"
fi
install -d -m 0755 -o root -g root "$CUA_SYNCED_TOKEN_DIR"
echo "[$(date -Iseconds)] token sync: $CUA_CLAIM_TOKEN_FILE -> $CUA_SYNCED_TOKEN_FILE ($DESKTOP_USER)"
exec "$BIN" token-sync --from "$CUA_CLAIM_TOKEN_FILE" --to "$CUA_SYNCED_TOKEN_FILE" --owner "$DESKTOP_USER"

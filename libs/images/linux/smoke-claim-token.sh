#!/usr/bin/env bash
# Smoke test for the Fleet per-claim token path (await-token-file mode) of
# linux, container rootfs variant, under runc or runsc (gVisor).
#
# Simulates the pod: no CUA_ENV_TOKEN, a tmpfs mounted at /run/cua (Fleet
# mounts the claim Secret there read-only; here it is writable so the test can
# play the operator). While the driver runs, the test writes, rotates and
# empties /run/cua/env-token as root (0600, atomic rename, like a Secret
# update) and checks from the host over gRPC-Web (curl):
#   awaiting   GetCapabilities/Health answer; Stat is FAILED_PRECONDITION with
#              and without a token
#   install    the token works, no token is UNAUTHENTICATED; the root
#              token-sync helper keeps /run/cua-env/env-token (cua 0600, dir
#              root 0755); the token is in no argv
#   rotate     the new token works, the old one is UNAUTHENTICATED
#   revoke     emptied file -> FAILED_PRECONDITION again (awaiting)
#   no network token   Init with a token while awaiting is refused
#
# Usage: smoke-claim-token.sh [--image REF] [--runtime runc|runsc] [--evidence DIR] [--keep]
set -euo pipefail
host_arch() { case "$(uname -m)" in arm64|aarch64) echo arm64 ;; *) echo amd64 ;; esac; }
IMAGE="cua-e2e-local/linux:docker-local-$(host_arch)"
RUNTIME=runc
EVIDENCE="${SMOKE_EVIDENCE:-$PWD/smoke-evidence}"
KEEP=0
while [ $# -gt 0 ]; do
    case "$1" in
        --image) IMAGE="$2"; shift 2 ;;
        --runtime) RUNTIME="$2"; shift 2 ;;
        --evidence) EVIDENCE="$2"; shift 2 ;;
        --keep) KEEP=1; shift ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
mkdir -p "$EVIDENCE"
NAME="cua-e2e-claim-token-$RUNTIME-$$"
TOKEN_A="claimA-$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')"
TOKEN_B="claimB-$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')"
PASS=0; FAIL=0
t_start=$SECONDS
WORK="$(mktemp -d "${TMPDIR:-/tmp}/cua-e2e-claim.XXXXXX")"

ok() { PASS=$((PASS + 1)); echo "  ok   $*"; }
bad() { FAIL=$((FAIL + 1)); echo "  FAIL $*"; }
cleanup() {
    docker exec "$NAME" sh -c 'tail -n 60 /var/log/supervisor/cua-spacesd.log /var/log/supervisor/cua-env-token-sync.log' \
        >"$EVIDENCE/$RUNTIME-claim-token-logs.txt" 2>&1 || true
    [ "$KEEP" = 1 ] || docker rm -f "$NAME" >/dev/null 2>&1 || true
    rm -rf "$WORK"
}
trap cleanup EXIT

echo "==> $IMAGE under --runtime=$RUNTIME (claim token simulation)"
docker run -d --name "$NAME" --runtime="$RUNTIME" --shm-size=512m \
    --memory="${SMOKE_MEMORY:-4g}" --memory-swap="${SMOKE_MEMORY:-4g}" \
    --tmpfs /run/cua:rw,mode=0755 \
    -e CUA_AUDIO=false \
    -p 127.0.0.1::3211 "$IMAGE" >/dev/null
PORT="$(docker port "$NAME" 3211/tcp | head -1 | sed 's/.*://')"
URL="http://127.0.0.1:$PORT"

# Writes the claim token as the operator would (root 0600, atomic rename).
# The token travels in the exec environment, not argv.
set_token() {
    docker exec -u root -e T="$1" "$NAME" sh -c \
        'umask 077; printf "%s" "$T" >/run/cua/.env-token.tmp && mv -f /run/cua/.env-token.tmp /run/cua/env-token'
}

clear_token() {
    docker exec -u root "$NAME" sh -c \
        ': >/run/cua/.env-token.tmp && chmod 600 /run/cua/.env-token.tmp && mv -f /run/cua/.env-token.tmp /run/cua/env-token'
}
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/claim-token-checks.sh"

echo "==> waiting for :3211"
wait_driver 120 && ok "driver answers GetCapabilities after $((SECONDS - t_start))s" || { bad "driver never answered"; exit 1; }

echo "==> awaiting (no claim bound)"
claim_awaiting_checks
docker exec "$NAME" sh -c 'grep -q "awaiting token file" /var/log/supervisor/cua-spacesd.log' \
    && ok "driver started in await-token-file mode" || bad "driver log lacks await-token-file mode"

echo "==> claim binds: token A"
claim_install_checks
perm="$(docker exec "$NAME" stat -c '%U %a' /run/cua-env/env-token 2>/dev/null || true)"
dperm="$(docker exec "$NAME" stat -c '%U %a' /run/cua-env 2>/dev/null || true)"
[ "$perm" = "cua 600" ] && [ "$dperm" = "root 755" ] \
    && ok "synced copy is cua 0600 in a root 0755 dir" || bad "synced copy perms: file '$perm' dir '$dperm'"
# The token reaches the checker via its environment (awk reads ENVIRON), so
# the checker's own argv never holds it.
if docker exec -e T="$TOKEN_A" "$NAME" sh -c \
    'for f in /proc/[0-9]*/cmdline; do tr "\0" " " <"$f" 2>/dev/null; echo; done | awk "index(\$0, ENVIRON[\"T\"]) { found = 1 } END { exit !found }"'; then
    bad "token appears in a process argv"
else
    ok "token in no argv"
fi

claim_rotate_revoke_checks

mem="$(docker stats --no-stream --format '{{.MemUsage}}' "$NAME" 2>/dev/null || true)"
echo "==> $RUNTIME claim-token smoke: $PASS passed, $FAIL failed in $((SECONDS - t_start))s (mem $mem)"
[ "$FAIL" = 0 ]

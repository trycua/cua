#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Live site-login e2e against a Linux container Space.
#
# Starts the Spaces image under docker (--memory=4g), maps
# login.example.test to the container's own loopback, serves a tiny login
# site inside the Space (tests/e2e/login-site.py, credentials known only to
# the site and the synthetic Chrome profile the test writes), publishes the
# spacesd on a loopback port and runs `cargo test -p cua-spaces-ext --test
# site_login_e2e`:
#
#   1. a synthetic Chrome profile (temp home, read through a FakeHost; the
#      real Chrome profile, Keychain and ~/.cua are never touched) is imported
#      into a passphrase Keyvault in a temp dir;
#   2. request_site_login without approval: pending, then declined, and the
#      site records no sign-in attempt;
#   3. request_site_login approved (a fake presence gate stands in for the
#      user's Touch ID): the Keyvault opens its own browser in the Space,
#      checks the origin, types the login through cua-driver and submits; the
#      site (queried inside the Space) reports the user signed in;
#   4. the agent's own tab: the test opens the site through the Spaces MCP
#      call_tool like an agent would, passes its tab, approves, and the site
#      reports a second sign-in.
#
# The container is always removed.
#
#   run-site-login-e2e.sh [--image REF] [--evidence DIR] [--keep] [--test NAME]
#
# --test picks the test target (default site_login_e2e);
# spaces_daemon_boundary runs the same site login, and a session teleport,
# through the MIT SDK and the Cua Spaces daemon extension (one per container:
# the Space's browser state carries over between them).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE="$(cd "$HERE/../../../.." && pwd)" # libs/cua
host_arch() { case "$(uname -m)" in arm64|aarch64) echo arm64 ;; *) echo amd64 ;; esac; }
IMAGE="cua-e2e-local/linux:docker-local-$(host_arch)"
KEEP=0
EVIDENCE=""
TEST=site_login_e2e
while [ $# -gt 0 ]; do
    case "$1" in
        --image) IMAGE="$2"; shift 2 ;;
        --evidence) EVIDENCE="$2"; shift 2 ;;
        --keep) KEEP=1; shift ;;
        --test) TEST="$2"; shift 2 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
EVIDENCE="${EVIDENCE:-$(mktemp -d)}"
mkdir -p "$EVIDENCE"
NAME="cua-e2e-site-login-$$"
TOKEN="$(head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n')"
# The site's credentials: random per run, known to the site and written
# into the synthetic Chrome profile by the test. Never printed.
SITE_USER="ada@example.test"
SITE_PASSWORD="kv-$(head -c 12 /dev/urandom | od -An -tx1 | tr -d ' \n')"
ENV_FILE="$(mktemp)"
chmod 600 "$ENV_FILE"
printf 'CUA_ENV_TOKEN=%s\n' "$TOKEN" >"$ENV_FILE"

cleanup() {
    rm -f "$ENV_FILE"
    docker exec "$NAME" sh -c 'tail -n 300 /var/log/supervisor/cua-spacesd.log' \
        >"$EVIDENCE/spacesd.log" 2>&1 || true
    docker logs "$NAME" >"$EVIDENCE/container.log" 2>&1 || true
    [ "$KEEP" = 1 ] || docker rm -f "$NAME" >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "==> $IMAGE as $NAME"
docker run -d --name "$NAME" --memory=4g --memory-swap=4g --shm-size=512m \
    --add-host login.example.test:127.0.0.1 \
    --env-file "$ENV_FILE" -p 127.0.0.1::3211 "$IMAGE" >/dev/null

status=""
for _ in $(seq 1 120); do
    status="$(docker inspect -f '{{.State.Health.Status}}' "$NAME" 2>/dev/null || echo gone)"
    [ "$status" = healthy ] || [ "$status" = gone ] && break
    sleep 1
done
[ "$status" = healthy ] || { echo "container health: $status" >&2; exit 1; }
PORT="$(docker port "$NAME" 3211/tcp | head -1 | sed 's/.*://')"
for _ in $(seq 1 60); do
    [ "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/health")" = 204 ] && break
    sleep 1
done

# The login site, inside the Space (loopback only; the browser in the Space
# reaches it as http://login.example.test:8000).
docker cp "$HERE/login-site.py" "$NAME:/tmp/login-site.py"
docker exec -d "$NAME" python3 /tmp/login-site.py "$SITE_USER" "$SITE_PASSWORD"
for _ in $(seq 1 30); do
    docker exec "$NAME" curl -s -o /dev/null http://login.example.test:8000/status && break
    sleep 1
done
echo "==> spacesd on 127.0.0.1:$PORT; site on login.example.test:8000 in the Space; evidence in $EVIDENCE"

cd "$WORKSPACE"
CUA_SITE_LOGIN_E2E_URL="http://127.0.0.1:$PORT" CUA_SITE_LOGIN_E2E_TOKEN="$TOKEN" \
    CUA_SITE_LOGIN_E2E_CONTAINER="$NAME" \
    CUA_SITE_LOGIN_E2E_USER="$SITE_USER" CUA_SITE_LOGIN_E2E_PASSWORD="$SITE_PASSWORD" \
    CUA_SITE_LOGIN_E2E_EVIDENCE="$EVIDENCE" \
    CUA_ENV_TEST_SANDBOX=1 CUA_CREDENTIAL_STORE=file \
    CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" \
    timeout 1800 cargo test -p cua-spaces-ext --test "$TEST" -- --test-threads=1 --nocapture 2>&1 \
    | sed "s/$SITE_PASSWORD/***/g" | tee "$EVIDENCE/cargo-test.log"
exit "${PIPESTATUS[0]}"

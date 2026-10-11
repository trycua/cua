#!/usr/bin/env bash
# Full relay account flow in Docker:
#
#   fake OIDC issuer ──┐
#   cua-relay (account mode, --oidc-issuer) ── internal network ── host:
#     linux running `cua-spacesd join` in account mode with
#     NO published ports and no route out except the relay
#   client (this machine): `cua auth login` against the fake issuer (temp
#     HOME, file credential store), `cua spaces ls`, then
#     `cargo test -p cua-spaces-ext --test relay_account_e2e`: relay_machines()
#     lists the host → connect → run a command → desktop stream keyframe →
#     presence shows the account user → host-side stop sharing → the client
#     is cut off and refused;
#   devices: a fresh `cua auth login` enrolls this device with no approval,
#     a new key of the same machine replaces the old record (one device,
#     `device_rekeyed` audited), and signing in again re-registers it.
#
# The host is registered the way `cua host setup` does it (POST
# /v1/machines with the owner's account token; the machine token and host
# policy are handed to the driver as files).
#
#   run-relay-account-e2e.sh [--image REF] [--keep] [--skip-build]
#
# Needs the Linux binaries from
# libs/cua-spacesd/scripts/ci/build-linux-e2e-bins.sh (run unless
# --skip-build). Every container gets --memory=4g; all are removed on exit.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE="$(cd "$HERE/../../../.." && pwd)" # libs/cua
REPO="$(cd "$WORKSPACE/../.." && pwd)"
host_arch() { case "$(uname -m)" in arm64|aarch64) echo arm64 ;; *) echo amd64 ;; esac; }
IMAGE="cua-e2e-local/linux:docker-local-$(host_arch)"
KEEP=0
BUILD=1
while [ $# -gt 0 ]; do
    case "$1" in
        --image) IMAGE="$2"; shift 2 ;;
        --keep) KEEP=1; shift ;;
        --skip-build) BUILD=0; shift ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
TARGET_VOLUME="${CUA_ENV_TARGET_VOLUME:-cua-e2e-host-target}"
RUN="cua-e2e-relayacct-$$"
INTERNAL="$RUN-internal"
EDGE="$RUN-edge"
MEM=(--memory=4g --memory-swap=4g)
EVIDENCE="${EVIDENCE:-$(mktemp -d)}"
CLIENT_HOME="$(mktemp -d)"
mkdir -p "$HOME/.cache"
MACHINE_ID="e2e$(od -An -N12 -tx1 /dev/urandom | tr -d ' \n')"

cleanup() {
    status=$?
    for c in issuer relay host; do
        docker logs "$RUN-$c" >"$EVIDENCE/$c.log" 2>&1 || true
    done
    docker exec "$RUN-host" sh -c 'tail -n 300 /var/log/supervisor/cua-spacesd.log' >"$EVIDENCE/spacesd.log" 2>&1 || true
    if [ "$status" != 0 ]; then
        echo "---- evidence in $EVIDENCE"
        tail -n 40 "$EVIDENCE/spacesd.log" "$EVIDENCE/relay.log" 2>/dev/null || true
    fi
    if [ "$KEEP" != 1 ]; then
        docker rm -f "$RUN-issuer" "$RUN-relay" "$RUN-host" >/dev/null 2>&1 || true
        docker network rm "$INTERNAL" "$EDGE" >/dev/null 2>&1 || true
    fi
    rm -rf "$CLIENT_HOME" "${WRAPPER_DIR:-}"
}
trap cleanup EXIT

if [ "$BUILD" = 1 ]; then
    CUA_ENV_TARGET_VOLUME="$TARGET_VOLUME" "$REPO/libs/cua-spacesd/scripts/ci/build-linux-e2e-bins.sh"
fi

docker network create --internal "$INTERNAL" >/dev/null
docker network create "$EDGE" >/dev/null

echo "==> fake OIDC issuer (user ada)"
docker run -d --name "$RUN-issuer" "${MEM[@]}" --network "$EDGE" --network-alias issuer -p 127.0.0.1::9000 \
    -v "$TARGET_VOLUME:/target:ro" cua-spacesd-linuxtest \
    /target/e2e/fake_oidc --listen 0.0.0.0:9000 --issuer http://issuer:9000 --sub ada --email ada@example.com >/dev/null
docker network connect --alias issuer "$INTERNAL" "$RUN-issuer"

echo "==> cua-relay (account mode)"
docker run -d --name "$RUN-relay" "${MEM[@]}" --network "$EDGE" --network-alias relay -p 127.0.0.1::8080 \
    -v "$TARGET_VOLUME:/target:ro" -e RUST_LOG=info,cua_relay=debug cua-spacesd-linuxtest \
    /target/e2e/cua-relay --listen 0.0.0.0:8080 --oidc-issuer http://issuer:9000 >/dev/null
docker network connect --alias relay "$INTERNAL" "$RUN-relay"

ISSUER_URL="http://$(docker port "$RUN-issuer" 9000/tcp | head -1)"
# Loopback: clients only send account tokens over plain http to localhost.
RELAY_URL="http://$(docker port "$RUN-relay" 8080/tcp | head -1 | sed 's/^0\.0\.0\.0:/127.0.0.1:/')"
for _ in $(seq 1 100); do
    curl -fsS -o /dev/null "$RELAY_URL/healthz" && curl -fsS -o /dev/null "$ISSUER_URL/healthz" && break
    sleep 0.2
done
echo "    issuer $ISSUER_URL, relay $RELAY_URL"

echo "==> register the host (what \`cua host setup\` does)"
OWNER_TOKEN="$(curl -fsS "$ISSUER_URL/mint" | python3 -c 'import json,sys; print(json.load(sys.stdin)["access_token"])')"
REG="$(curl -fsS -X POST "$RELAY_URL/v1/machines" -H "authorization: Bearer $OWNER_TOKEN" \
    -H 'content-type: application/json' -d "{\"id\":\"$MACHINE_ID\",\"name\":\"e2e desktop\"}")"
MACHINE_TOKEN="$(printf '%s' "$REG" | python3 -c 'import json,sys; print(json.load(sys.stdin)["machine_token"])')"
RELAY_JWKS="$(printf '%s' "$REG" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin)["jwks"]))')"

# The driver starts through the image's start-spacesd.sh (user cua);
# this wrapper lays down the host files, then execs `join`.
# Under $HOME: Colima / Docker Desktop only share the user's home with the VM.
WRAPPER_DIR="$(mktemp -d "${HOME}/.cache/cua-e2e-relayacct.XXXXXX")"
WRAPPER="$WRAPPER_DIR/host-join.sh"
cat >"$WRAPPER" <<'EOS'
#!/usr/bin/env bash
set -euo pipefail
dir="$HOME/.cua/host"
mkdir -p "$dir" "$HOME/.cua/spacesd"
umask 077
printf '%s\n' "$CUA_E2E_MACHINE_TOKEN" >"$dir/machine-token"
printf '{"owner":"ada","owner_email":"ada@example.com","allow":[],"sharing":true}\n' >"$dir/host.json"
printf '%s\n' "$CUA_E2E_MACHINE_ID" >"$HOME/.cua/spacesd/id"
printf '%s\n' "$CUA_E2E_RELAY_JWKS" >"$dir/relay-jwks.json"
exec /target/e2e/cua-spacesd join --relay ws://relay:8080 --relay-token-file "$dir/machine-token" \
    --host-policy "$dir/host.json" --machine-id-file "$HOME/.cua/spacesd/id" \
    --relay-jwks "$dir/relay-jwks.json" --heartbeat-secs 3 "$@"
EOS
chmod 755 "$WRAPPER"

echo "==> host: $IMAGE, internal network only, no published ports"
docker create --name "$RUN-host" "${MEM[@]}" --shm-size=512m --network "$INTERNAL" \
    -v "$TARGET_VOLUME:/target:ro" -v "$WRAPPER:/opt/cua-e2e/host-join.sh:ro" \
    -e CUA_SPACESD_BIN=/opt/cua-e2e/host-join.sh -e CUA_E2E_MACHINE_ID="$MACHINE_ID" \
    -e CUA_E2E_MACHINE_TOKEN="$MACHINE_TOKEN" -e CUA_E2E_RELAY_JWKS="$RELAY_JWKS" -e CUA_AUDIO=false -e CUA_NOVNC=false \
    -e CUA_ENV_LOG=info,cua_relay=debug "$IMAGE" >/dev/null
docker start "$RUN-host" >/dev/null
if [ -n "$(docker port "$RUN-host")" ]; then echo "host publishes ports" >&2; exit 1; fi
online=0
for _ in $(seq 1 120); do
    if curl -fsS "$RELAY_URL/v1/machines/$MACHINE_ID" -H "authorization: Bearer $MACHINE_TOKEN" | grep -q '"online":true'; then
        online=1; break
    fi
    sleep 1
done
[ "$online" = 1 ] || { echo "host never joined the relay" >&2; exit 1; }
echo "    host $MACHINE_ID online"

echo "==> client sign-in: cua auth login (fake issuer, temp HOME, file store)"
(cd "$WORKSPACE" && CARGO_BUILD_JOBS=4 cargo build -q -p cua-cli)
CUA="$WORKSPACE/target/debug/cua"
# Keep cargo/rustup on the real home; only the cua client gets the temp one.
# docker's client config (the colima context) too, or cleanup cannot reach
# the engine.
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" \
    DOCKER_CONFIG="${DOCKER_CONFIG:-$HOME/.docker}"
export HOME="$CLIENT_HOME" CUA_HOME="$CLIENT_HOME/.cua" CUA_CREDENTIAL_STORE=file CUA_NO_BROWSER=1 \
    CUA_OIDC_ISSUER="$ISSUER_URL" CUA_OIDC_POLL_UNIT_MS=100 CUA_RELAY_URL="$RELAY_URL"
"$CUA" auth login --no-browser </dev/null
test -s "$CUA_HOME/credentials.json"
if "$CUA" spaces --help >/dev/null 2>&1; then
    echo "==> cua spaces ls"
    "$CUA" spaces ls | tee "$EVIDENCE/spaces-ls.txt"
    grep -q "relay:$MACHINE_ID" "$EVIDENCE/spaces-ls.txt"
fi

echo "==> SDK flow"
(cd "$WORKSPACE" && CUA_RELAY_E2E_URL="$RELAY_URL" CUA_RELAY_E2E_CREDENTIALS="$CUA_HOME/credentials.json" \
    CUA_RELAY_E2E_MACHINE="$MACHINE_ID" CUA_RELAY_E2E_MACHINE_TOKEN="$MACHINE_TOKEN" CARGO_BUILD_JOBS=4 \
    timeout 900 cargo test -p cua-spaces-ext --test relay_account_e2e -- --nocapture --test-threads=1)
echo "==> devices: a fresh sign-in enrolls; a new key of this machine replaces the old one"
json_field() { python3 -c "import json,sys; v=json.load(sys.stdin)
for k in sys.argv[1].split('.'): v=v[k]
print(v)" "$1"; }
"$CUA" auth login --no-browser </dev/null >/dev/null
"$CUA" --json devices enroll >"$EVIDENCE/enroll-1.json"
[ "$(json_field device.state <"$EVIDENCE/enroll-1.json")" = enrolled ] || { cat "$EVIDENCE/enroll-1.json"; exit 1; }
OLD_ID="$(json_field device.id <"$EVIDENCE/enroll-1.json")"
# Another build of cua on this machine keeps its own key: set this one aside.
mv "$CUA_HOME/device-key" "$CUA_HOME/device-key.other-build"
"$CUA" --json devices enroll >"$EVIDENCE/enroll-2.json"
[ "$(json_field device.state <"$EVIDENCE/enroll-2.json")" = enrolled ] || { cat "$EVIDENCE/enroll-2.json"; exit 1; }
NEW_ID="$(json_field device.id <"$EVIDENCE/enroll-2.json")"
[ "$NEW_ID" != "$OLD_ID" ]
python3 -c "import json,sys; assert json.load(open(sys.argv[1]))['superseded'] == [sys.argv[2]]" \
    "$EVIDENCE/enroll-2.json" "$OLD_ID"
"$CUA" --json devices ls >"$EVIDENCE/devices-ls.json"
python3 -c "import json,sys; ids=[d['id'] for d in json.load(open(sys.argv[1]))['devices']]; assert ids == [sys.argv[2]], ids" \
    "$EVIDENCE/devices-ls.json" "$NEW_ID"
"$CUA" devices audit --limit 20 | tee "$EVIDENCE/devices-audit.txt"
grep -q "device_rekeyed  device $NEW_ID  $OLD_ID" "$EVIDENCE/devices-audit.txt"
grep -q "enrolled by fresh sign-in" "$EVIDENCE/devices-audit.txt"
# Signing in again re-registers this device (no code, no other device).
"$CUA" auth login --no-browser </dev/null | tee "$EVIDENCE/login-again.txt"
grep -q "This device ($NEW_ID) is enrolled until" "$EVIDENCE/login-again.txt"
echo "==> relay account flow: PASS (evidence: $EVIDENCE)"

#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Reverse-tunnel lane: cua-spacesd joins a cua-relay from a container
# with NO published ports on an internal Docker network; the conformance
# suite runs from a third container through http://relay:8080/m/<id>
# (native gRPC, gRPC-Web, 32+ parallel streams, 1 GiB transfers, tunnels).
# Then the relay container is killed mid-stream and restarted: the machine
# rejoins and ConnectProcess by tag replays scrollback.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../../.." && pwd)"
image="${CUA_ENV_TEST_IMAGE:-cua-spacesd-linuxtest}"
target_volume="${CUA_ENV_TARGET_VOLUME:-cua-envcore-target}"
cargo_volume="${CUA_ENV_CARGO_VOLUME:-cua-envcore-cargo}"
run_id="cua-relay-e2e-$$"
net="$run_id"
env_token="env-$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')"
relay_token="relay-$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')"
machine_id="e2e$(od -An -N12 -tx1 /dev/urandom | tr -d ' \n')"
mem=(--memory=4g --memory-swap=4g)

cleanup() {
  status=$?
  if [ "$status" != 0 ]; then
    for c in relay machine; do
      echo "---- $c logs (tail)"; docker logs --tail 80 "$run_id-$c" 2>&1 || true
    done
  fi
  docker rm -f "$run_id-relay" "$run_id-machine" >/dev/null 2>&1 || true
  docker network rm "$net" >/dev/null 2>&1 || true
}
trap cleanup EXIT

docker image inspect "$image" >/dev/null 2>&1 || docker build -t "$image" -f "$here/linux-test.Dockerfile" "$here"

echo "== building binaries and the conformance test"
# Big debug test binaries OOM GNU ld at 4 GiB: line-tables-only debug info,
# and rust-lld on aarch64 (scripts/ci/linux/cc-rust-lld.sh; x86_64 already
# links with rust-lld).
docker run --rm "${mem[@]}" -v "$repo:/repo" -v "$target_volume:/target" -v "$cargo_volume:/usr/local/cargo/registry" \
  -e CARGO_TARGET_DIR=/target -e CARGO_BUILD_JOBS=4 -e CARGO_PROFILE_DEV_DEBUG="${CARGO_PROFILE_DEV_DEBUG:-line-tables-only}" -e CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=/repo/scripts/ci/linux/cc-rust-lld.sh \
  -w /repo/libs/cua-spacesd "$image" bash -c '
    set -euo pipefail
    cargo build --locked -p cua-spacesd -p cua-relay
    test_bin=$(cargo test --locked -p cua-spacesd-server --test conformance --no-run --message-format=json \
      | python3 -c "import json,sys; print([m[\"executable\"] for m in map(json.loads, sys.stdin) if m.get(\"reason\")==\"compiler-artifact\" and m.get(\"executable\") and m[\"target\"][\"name\"]==\"conformance\"][-1])")
    mkdir -p /target/e2e
    cp /target/debug/cua-spacesd /target/debug/cua-relay "$test_bin" /target/e2e/
    mv /target/e2e/$(basename "$test_bin") /target/e2e/conformance
  '

echo "== image invocation: --listen 0.0.0.0:3211 with the token from env, file, or nowhere"
docker run --rm "${mem[@]}" -v "$target_volume:/target:ro" "$image" bash -c '
  set -u
  check() { # name, then how to start
    name=$1; shift
    "$@" & pid=$!
    for _ in $(seq 1 50); do curl -fsS -o /dev/null http://127.0.0.1:3211/health && break; sleep 0.2; done
    code=$(curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:3211/health)
    kill $pid 2>/dev/null; wait $pid 2>/dev/null
    [ "$code" = 204 ] || { echo "$name: /health=$code" >&2; exit 1; }
    echo "$name: ok"
  }
  check env env CUA_ENV_TOKEN=t1 /target/e2e/cua-spacesd --listen 0.0.0.0:3211 --no-driver
  mkdir -p /run/cua && echo t2 > /run/cua/env-token
  check file /target/e2e/cua-spacesd --listen 0.0.0.0:3211 --no-driver
  rm /run/cua/env-token
  if /target/e2e/cua-spacesd --listen 0.0.0.0:3211 --no-driver 2>/dev/null; then echo "no token: served" >&2; exit 1; fi
  echo "no token: refused"
'

docker network create --internal "$net" >/dev/null
start_relay() {
  docker run -d --name "$run_id-relay" --network "$net" --network-alias relay "${mem[@]}" \
    -v "$target_volume:/target:ro" -e CUA_RELAY_TOKENS="$relay_token" -e RUST_LOG=info,cua_relay=debug \
    "$image" /target/e2e/cua-relay --listen 0.0.0.0:8080 >/dev/null
}
start_relay
echo "== machine (no published ports)"
docker run -d --name "$run_id-machine" --network "$net" "${mem[@]}" \
  -v "$target_volume:/target:ro" -e CUA_ENV_TOKEN="$env_token" -e CUA_RELAY_TOKEN="$relay_token" -e CUA_ENV_LOG=info,cua_relay=debug \
  "$image" bash -c "mkdir -p /root/.cua/spacesd && echo $machine_id > /root/.cua/spacesd/id && \
    exec /target/e2e/cua-spacesd join --relay ws://relay:8080 --heartbeat-secs 3" >/dev/null
if [ -n "$(docker port "$run_id-machine")" ]; then echo "machine publishes ports" >&2; exit 1; fi

runner() { # extra docker -e args..., then test args
  docker run --rm --network "$net" "${mem[@]}" -v "$target_volume:/target:ro" \
    -e CUA_ENV_TEST_TARGET="http://relay:8080/m/$machine_id" -e CUA_ENV_TEST_TOKEN="$env_token" \
    -e CUA_ENV_TEST_BIG_BYTES="${CUA_ENV_TEST_BIG_BYTES:-1073741824}" \
    -e CUA_ENV_TEST_LONG_SECS="${CUA_ENV_TEST_LONG_SECS:-60}" "$@"
}

echo "== waiting for the machine to join"
for _ in $(seq 1 60); do
  if runner "$image" curl -fsS -o /dev/null "http://relay:8080/m/$machine_id/health"; then break; fi
  sleep 1
done

echo "== conformance suite through the relay"
runner "$image" timeout 1800 /target/e2e/conformance --test-threads=4

echo "== kill the relay mid-stream, then reattach by tag"
tag="relay-kill-$$"
runner -e CUA_ENV_TEST_PHASE=start -e CUA_ENV_TEST_PHASE_TAG="$tag" -e CUA_ENV_TEST_LONG_SECS=20 \
  "$image" /target/e2e/conformance phased_relay_kill_reattach --exact
docker kill "$run_id-relay" >/dev/null
docker rm "$run_id-relay" >/dev/null
sleep 2
start_relay
runner -e CUA_ENV_TEST_PHASE=reattach -e CUA_ENV_TEST_PHASE_TAG="$tag" -e CUA_ENV_TEST_LONG_SECS=20 \
  "$image" timeout 300 /target/e2e/conformance phased_relay_kill_reattach --exact
echo "== relay e2e passed"

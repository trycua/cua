#!/usr/bin/env bash
# Live Spaces e2e against a linux container.
#
# Starts the image under docker (runc, or runsc = gVisor) with --memory=4g,
# publishes the spacesd on a loopback port, then runs
# `cargo test -p cua-spaces --test e2e_docker` (bash, send_file (sha256
# checked by the guest's own sha256sum), window and desktop streams (frames
# arrive, keyframe first), presence with two clients, hotspot egress, and the
# agent runner attaching), then from cua-spaces-ext (they ship with Cua
# Spaces) `e2e_docker_stream` (a window and the desktop streamed, viewer
# input attributed to its presence) and `e2e_docker_teleport` (a synthetic
# Firefox profile teleport). The container is always removed.
#
#   run-docker-e2e.sh [--image REF] [--driver PATH] [--runtime runc|runsc] [--keep]
#                     [--test e2e_docker|e2e_docker_stream|e2e_docker_teleport|e2e_teleport_app|bench_home]
#                     [--release]
#
#   --release  build the test in release (benchmarks: bench_home)
#
#   --test     one test target instead of the default pair (e2e_teleport_app
#              runs "Teleport an app..." of a fixture app: a pinned install,
#              files, launch, all verified in the guest). The teleport and
#              bench targets are in cua-spaces-ext.
#
#   --image    default cua-e2e-local/linux:docker-local-<arch>
#              (build it: libs/images/build.sh linux)
#   --driver   a Linux cua-spacesd binary to bind-mount over the image's
#              (for testing a driver newer than the image), or `commit` to
#              build this checkout's (libs/images/linux/build-spacesd-linux.sh)
#              and mount that. Without it the image's own spacesd runs, and
#              the script warns when that is not this checkout's build: the
#              suites then test the image's older server (for example, one
#              without viewer input attribution refuses no second viewer).
#   --evidence directory for container logs (default: a temp dir)
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE="$(cd "$HERE/../../../.." && pwd)" # libs/cua
host_arch() { case "$(uname -m)" in arm64|aarch64) echo arm64 ;; *) echo amd64 ;; esac; }
IMAGE="cua-e2e-local/linux:docker-local-$(host_arch)"
DRIVER=""
RUNTIME=runc
KEEP=0
EVIDENCE=""
TEST=
PROFILE=()
while [ $# -gt 0 ]; do
    case "$1" in
        --image) IMAGE="$2"; shift 2 ;;
        --driver) DRIVER="$2"; shift 2 ;;
        --runtime) RUNTIME="$2"; shift 2 ;;
        --evidence) EVIDENCE="$2"; shift 2 ;;
        --keep) KEEP=1; shift ;;
        --test) TEST="$2"; shift 2 ;;
        --release) PROFILE=(--release); shift ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
EVIDENCE="${EVIDENCE:-$(mktemp -d)}"
mkdir -p "$EVIDENCE"
NAME="cua-e2e-spaces-$$"
TOKEN="$(head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n')"
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

REPO_ROOT="$(cd "$WORKSPACE/../.." && pwd)"
if [ "$DRIVER" = commit ]; then
    echo "==> building cua-spacesd for linux/$(host_arch) from this checkout"
    "$REPO_ROOT/libs/images/linux/build-spacesd-linux.sh" "$(host_arch)"
    DRIVER="$REPO_ROOT/libs/images/linux/dist/$(host_arch)/cua-spacesd"
fi
mount=()
if [ -z "$DRIVER" ]; then
    image_sha="$(docker run --rm --entrypoint /usr/local/bin/cua-spacesd "$IMAGE" build-info 2>/dev/null \
        | sed -n 's/.*"git_sha": *"\([0-9a-f]*\)".*/\1/p' | head -1)"
    head_sha="$(git -C "$REPO_ROOT" rev-parse HEAD 2>/dev/null || true)"
    if [ -z "$image_sha" ] || [ "$image_sha" != "$head_sha" ]; then
        echo "warning: $IMAGE runs cua-spacesd ${image_sha:-of an unknown build}, not this checkout ($head_sha);" >&2
        echo "warning: pass --driver commit to test this checkout's server" >&2
    fi
fi
if [ -n "$DRIVER" ]; then
    mount=(-v "$(cd "$(dirname "$DRIVER")" && pwd)/$(basename "$DRIVER"):/usr/local/bin/cua-spacesd:ro")
fi
echo "==> $IMAGE ($RUNTIME${DRIVER:+, driver $DRIVER}) as $NAME"
docker run -d --name "$NAME" --runtime="$RUNTIME" --memory=4g --memory-swap=4g --shm-size=512m \
    --env-file "$ENV_FILE" -p 127.0.0.1::3211 ${mount[@]+"${mount[@]}"} "$IMAGE" >/dev/null

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
echo "==> spacesd on 127.0.0.1:$PORT; evidence in $EVIDENCE"

cd "$WORKSPACE"
# The teleport test prints per-phase timings and saves a screenshot.
extra=()
[ "$TEST" = e2e_teleport_app ] && extra=(--nocapture)
package_of() { case "$1" in e2e_docker) echo cua-spaces ;; *) echo cua-spaces-ext ;; esac; }
TESTS=("${TEST:-e2e_docker}")
[ -z "$TEST" ] && TESTS+=(e2e_docker_stream e2e_docker_teleport)
for t in "${TESTS[@]}"; do
    CUA_SPACES_E2E_URL="http://127.0.0.1:$PORT" CUA_SPACES_E2E_TOKEN="$TOKEN" \
        CUA_TELEPORT_E2E_EVIDENCE="$EVIDENCE" \
        CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" \
        timeout 1800 cargo test ${PROFILE[@]+"${PROFILE[@]}"} -p "$(package_of "$t")" --test "$t" -- --test-threads=2 ${extra[@]+"${extra[@]}"} 2>&1 | tee -a "$EVIDENCE/cargo-test.log"
    status="${PIPESTATUS[0]}"
    [ "$status" = 0 ] || exit "$status"
done

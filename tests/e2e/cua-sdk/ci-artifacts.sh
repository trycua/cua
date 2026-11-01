#!/usr/bin/env bash
# Hands the build jobs' outputs to the test shards of
# .github/workflows/e2e-cua-sdk.yml. Every archive is a tar.zst of paths
# relative to the repository root, so the shard sees the tree the single-job
# workflow used to build (same checkout path, same file modes).
#
#   ci-artifacts.sh pack sdk|cli|fixtures|rust-tests|images DIR
#       DIR/cua-e2e-build-<name>.tar.zst
#         sdk         the release library staged for Python and Node (+ node_modules)
#         cli         the debug cua CLI and the TypeScript + wasm browser builds
#         fixtures    cua-test-fixtures and the static cua-spacesd the suite
#                     injects into plain images
#         rust-tests  the prebuilt Rust test binaries
#         images      the reference images, as one `docker save`
#   ci-artifacts.sh prepare DIR
#       a shard's setup: gVisor, the archives of DIR unpacked, the images
#       loaded, and the canonical desktop image pulled while they load
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
STATIC_SPACESD=libs/cua-spacesd/target/x86_64-unknown-linux-musl/static/cua-spacesd

pack_paths() {
    local name="$1" out="$2"; shift 2
    local path
    for path in "$@"; do
        [ -e "$REPO/$path" ] || { echo "pack $name: $path was not built" >&2; exit 1; }
    done
    tar --zstd -cf "$out/cua-e2e-build-$name.tar.zst" -C "$REPO" "$@"
    ls -lh "$out/cua-e2e-build-$name.tar.zst"
}

pack() {
    local name="$1" out="$2"
    mkdir -p "$out"
    case "$name" in
        sdk)
            pack_paths sdk "$out" libs/cua/python/src/cua/libcua_sdk.so libs/cua/typescript/node_modules ;;
        cli)
            pack_paths cli "$out" libs/cua/target/debug/cua libs/cua/typescript/dist libs/cua/typescript/browser ;;
        fixtures)
            pack_paths fixtures "$out" libs/cua/target/debug/cua-test-fixtures "$STATIC_SPACESD" ;;
        rust-tests)
            pack_paths rust-tests "$out" tests/e2e/cua-sdk/rust/prebuilt ;;
        images)
            local refs
            refs="$(docker image ls --format '{{.Repository}}:{{.Tag}}' | grep '^cua-e2e-local/' || true)"
            [ "$(wc -w <<<"$refs")" -ge 3 ] || { echo "pack images: expected 3 images, found: $refs" >&2; exit 1; }
            # shellcheck disable=SC2086
            docker save $refs | zstd -T0 -3 -q -o "$out/cua-e2e-build-images.tar.zst"
            echo "$refs"
            ls -lh "$out/cua-e2e-build-images.tar.zst" ;;
        *) echo "unknown archive $name" >&2; exit 2 ;;
    esac
}

prepare() {
    local dir="$1"
    # gVisor registers a docker runtime and restarts the daemon, so it goes
    # first; the archives unpack while it installs.
    "$HERE/ci-setup.sh" --only system &
    local system=$!
    local name
    for name in sdk cli fixtures rust-tests; do
        tar --zstd -xf "$dir/cua-e2e-build-$name.tar.zst" -C "$REPO"
    done
    wait "$system"
    # The images load while the canonical desktop image (the one the docs
    # blocks start through Image.linux()) is pulled with the same resolver the
    # SDK uses, so the first docs block does not wait for the pull.
    local cua="$REPO/libs/cua/target/debug/cua"
    (zstd -dc "$dir/cua-e2e-build-images.tar.zst" | docker load) &
    local load=$!
    (timeout 900 "$cua" --embedded image pull container:ghcr.io/trycua/linux:24.04 ||
        echo "::warning::could not prefetch the canonical linux image; the first docs block pulls it") &
    local pull=$!
    local rc=0
    wait "$load" || rc=1
    wait "$pull" || true
    docker image ls --format '{{.Repository}}:{{.Tag}} {{.Size}}'
    return "$rc"
}

case "${1:-}" in
    pack) pack "${2:?name}" "${3:?dir}" ;;
    prepare) prepare "${2:?dir}" ;;
    *) sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//' >&2; exit 2 ;;
esac

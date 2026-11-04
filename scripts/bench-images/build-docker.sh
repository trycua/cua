#!/usr/bin/env bash
# Build both variants of a Dockerfile-defined benchmark (bench.json build.kind=dockerfile).
#
#   build-docker.sh ID [--platform linux/arm64,linux/amd64] [--outputs rootfs,disk] [--work DIR]
#
# 1. The base (bench.json build.base, e.g. linux) is built from this
#    checkout with its cua-spacesd (libs/images/linux/dist/<arch>), as
#    cua-e2e-local/<base>:docker-benchimg-<arch>, so the bench image runs the
#    same guest agent as the commit it is built from. bench.json
#    build.base_target picks the base's Dockerfile stage (the linux tier:
#    benchmarks build FROM slim, so they carry no developer tooling). CUA_BENCH_BASE_IMAGE
#    (a digest-pinned ref such as ghcr.io/trycua/linux@sha256:...) uses a
#    published base instead; it is passed as the Dockerfile's BASE_IMAGE.
# 2. libs/images/bench/ID/Dockerfile on that base, through libs/images/build.sh,
#    gives the rootfs (cua-e2e-local/bench-ID:docker-local-<arch>) and the
#    VM disk derived from it (WORK/ID/out/<arch>/disk.img), with the
#    bench.json labels on the rootfs config.
# One platform at a time (each needs its own base).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
ID="${1:?benchmark id}"; shift
WORK_ROOT="${CUA_BENCH_WORK:-$HOME/.cache/cua-bench-images}"
OUTPUTS="rootfs,disk"
PLATFORMS=""
while [ $# -gt 0 ]; do
    case "$1" in
        --platform) PLATFORMS="$2"; shift 2 ;;
        --outputs) OUTPUTS="$2"; shift 2 ;;
        --work) WORK_ROOT="$2"; shift 2 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
bench() { python3 "$HERE/bench.py" "$@"; }
log() { echo "[build-docker $ID $(date +%T)] $*" >&2; }
[ "$(bench get "$ID" build.kind)" = dockerfile ] || { echo "$ID is not a dockerfile benchmark" >&2; exit 2; }
if [ -z "$PLATFORMS" ]; then
    PLATFORMS="$(bench get "$ID" arch | python3 -c 'import json,sys; print(",".join("linux/"+a for a in json.load(sys.stdin)))')"
fi
BASE="$(bench get "$ID" build.base)"
BASE_TARGET="$(bench get "$ID" build.base_target 2>/dev/null || true)"
base_target_args=()
[ -z "$BASE_TARGET" ] || base_target_args=(--target "$BASE_TARGET")
WORK="$WORK_ROOT/$ID"
mkdir -p "$WORK/out"
ctx_args=()
while IFS= read -r c; do ctx_args+=(--build-context "$c"); done < <(bench get "$ID" build.build_contexts \
    | python3 -c 'import json,sys,os; r=sys.argv[1]; [print(f"{k}={os.path.join(r,v)}") for k,v in json.load(sys.stdin).items()]' "$REPO")
label_args=()
while IFS= read -r l; do label_args+=(--label "$l"); done < <(bench labels "$ID" --variant rootfs \
    | python3 -c 'import json,sys; [print(f"{k}={v}") for k,v in json.load(sys.stdin).items()]')

for platform in ${PLATFORMS//,/ }; do
    arch="${platform#linux/}"
    if [ -n "${CUA_BENCH_BASE_IMAGE:-}" ]; then
        base_ref="$CUA_BENCH_BASE_IMAGE"
        log "base $base_ref (published)"
    else
        [ -x "$REPO/libs/images/$BASE/dist/$arch/cua-spacesd" ] || [ "$BASE" != linux ] || {
            echo "missing libs/images/linux/dist/$arch/cua-spacesd; run build-spacesd-linux.sh $arch" >&2; exit 1; }
        log "base $BASE ($platform)"
        "$REPO/libs/images/build.sh" "$BASE" --platform "$platform" --tag benchimg --outputs rootfs \
            ${base_target_args[@]+"${base_target_args[@]}"}
        base_ref="cua-e2e-local/$(basename "$BASE"):docker-benchimg-$arch"
    fi
    log "bench-$ID ($platform, $OUTPUTS)"
    "$REPO/libs/images/build.sh" "bench/$ID" --platform "$platform" --repo "cua-e2e-local/bench-$ID" \
        --tag local --outputs "$OUTPUTS" --out "$WORK/build/disk" \
        --build-arg "BASE_IMAGE=$base_ref" ${ctx_args[@]+"${ctx_args[@]}"} "${label_args[@]}"
    if [[ ",$OUTPUTS," == *",disk,"* ]]; then
        mkdir -p "$WORK/out/$arch"
        ln -sf "$WORK/build/disk/$ID/$arch/disk.img" "$WORK/out/$arch/disk.img"
    fi
done
log "done"

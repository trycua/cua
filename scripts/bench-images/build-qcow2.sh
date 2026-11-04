#!/usr/bin/env bash
# Build both variants of a qcow2-sourced benchmark (bench.json build.kind=qcow2).
#
#   build-qcow2.sh ID [--stages prepare,vm,rootfs,container] [--work DIR]
#
# Stages (each is skipped when its output exists; delete it to redo):
#   prepare    overlay of the pinned upstream disk, e2fsck -fy     -> base-fixed.qcow2
#   vm         libs/images/bench/ID/vm/customize.sh               -> vm/disk.img
#   rootfs     guestfish tar-out of the prepared disk, split into
#              gzip layers by top-level dir (bench.json rootfs_layers),
#              minus rootfs_excludes                              -> rootfs/layers/*.tar.gz + base OCI layout
#   container  the upstream layers plus libs/images/bench/ID/container/overlay.py
#              (files only: supervisord, init scripts, spacesd)    -> out/<arch>/rootfs-layout (OCI layout)
#
# Inputs come from `bench.py fetch ID` (run first; verified by sha256). The
# guest agent is libs/images/linux/dist/<arch>/cua-spacesd
# (build-spacesd-linux.sh); it must need at most the guest's glibc.
# Everything that touches the disk runs in scripts/bench-images/guestfs,
# unprivileged, with /dev/kvm when the docker VM has it.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
ID="${1:?benchmark id}"; shift
STAGES="prepare,vm,rootfs,container"
WORK_ROOT="${CUA_BENCH_WORK:-$HOME/.cache/cua-bench-images}"
while [ $# -gt 0 ]; do
    case "$1" in
        --stages) STAGES="$2"; shift 2 ;;
        --work) WORK_ROOT="$2"; shift 2 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
want() { [[ ",$STAGES," == *",$1,"* ]]; }
log() { echo "[build-qcow2 $ID $(date +%T)] $*" >&2; }
bench() { python3 "$HERE/bench.py" "$@"; }

[ "$(bench get "$ID" build.kind)" = qcow2 ] || { echo "$ID is not a qcow2 benchmark" >&2; exit 2; }
ARCH="$(bench get "$ID" arch | python3 -c 'import json,sys; a=json.load(sys.stdin); assert len(a)==1, "qcow2 sources are single-arch"; print(a[0])')"
WORK="$WORK_ROOT/$ID"
SRC="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["image"])' "$WORK/sources.json")"
# Only the disk stages read the upstream disk (CI drops it before the
# container stage to save space).
if want prepare || want vm || want rootfs; then
    [ -f "$SRC" ] || { echo "missing $SRC; run: bench.py fetch $ID" >&2; exit 1; }
fi
SPACESD="$REPO/libs/images/linux/dist/$ARCH/cua-spacesd"
[ -x "$SPACESD" ] || { echo "missing $SPACESD; run libs/images/linux/build-spacesd-linux.sh $ARCH" >&2; exit 1; }
BUILD="$WORK/build"
mkdir -p "$BUILD/spacesd"
cp "$SPACESD" "$BUILD/spacesd/cua-spacesd"
HELPER="cua-e2e-local/bench-guestfs:1"
KVM=()
if want prepare || want vm || want rootfs; then
    docker image inspect "$HELPER" >/dev/null 2>&1 || docker build -q -t "$HELPER" "$HERE/guestfs"
    [ -e /dev/kvm ] && KVM=(--device /dev/kvm)
    # The docker VM may have /dev/kvm even when this host does not (Linux CI).
    docker run --rm "$HELPER" test -e /dev/kvm 2>/dev/null && KVM=(--device /dev/kvm)
fi
# Without KVM, libguestfs must be told to use TCG (on aarch64 it otherwise
# asks qemu for -cpu host, which exits at once).
ACCEL=(-e LIBGUESTFS_BACKEND_SETTINGS=force_tcg)
[ ${#KVM[@]} -gt 0 ] && ACCEL=()
guestfs() {
    docker run --rm --memory=4g --memory-swap=4g ${KVM[@]+"${KVM[@]}"} ${ACCEL[@]+"${ACCEL[@]}"} \
        -v "$REPO:/repo:ro" -v "$(dirname "$SRC"):/src:ro" -v "$BUILD:/work" \
        -e LIBGUESTFS_BACKEND=direct "$HELPER" "$@"
}
SRC_IN="/src/$(basename "$SRC")"

# /etc/cua-image/manifest.json for a variant (cua-image-manifest, from the
# benchmark's image.json and `cua-spacesd build-info` of the binary baked in,
# run once under the guest's distro and arch).
manifest_for() { # VARIANT OUT
    if [ ! -s "$BUILD/spacesd/build-info.json" ]; then
        docker run --rm --platform "linux/$ARCH" --memory=2g -v "$BUILD/spacesd:/g:ro" ubuntu:22.04 bash -c \
            'apt-get update -qq >/dev/null && apt-get install -y -qq --no-install-recommends libx11-6 libxi6 >/dev/null && /g/cua-spacesd build-info' \
            >"$BUILD/spacesd/build-info.json"
    fi
    python3 "$REPO/libs/images/common/tools/cua-image-manifest" generate \
        --image-json "$REPO/libs/images/bench/$ID/image.json" --variant "$1" --arch "$ARCH" \
        --spacesd-source local --spacesd /usr/local/bin/cua-spacesd --build-info "$BUILD/spacesd/build-info.json" \
        --source-revision "$(git -C "$REPO" rev-parse HEAD)" --out "$2"
}

if want prepare && [ ! -f "$BUILD/base-fixed.qcow2" ]; then
    log "prepare (e2fsck on an overlay of $(basename "$SRC"))"
    guestfs /repo/scripts/bench-images/qcow2-prepare.sh "$SRC_IN" /work/base-fixed.qcow2.tmp /work/base-fixed.fsck.txt
    mv "$BUILD/base-fixed.qcow2.tmp" "$BUILD/base-fixed.qcow2"
fi

if want vm && [ ! -f "$BUILD/vm/disk.img" ]; then
    log "vm variant"
    [ -f "$REPO/libs/images/bench/$ID/vm/customize.sh" ] || { echo "no vm/customize.sh" >&2; exit 1; }
    mkdir -p "$BUILD/vm"
    manifest_for containerdisk "$BUILD/vm/manifest.json"
    guestfs bash "/repo/libs/images/bench/$ID/vm/customize.sh" /work/base-fixed.qcow2 /work/spacesd/cua-spacesd /work/vm
fi
if [ -f "$BUILD/vm/disk.img" ]; then
    mkdir -p "$WORK/out/$ARCH"
    ln -sf "$BUILD/vm/disk.img" "$WORK/out/$ARCH/disk.img"
fi

if want rootfs && [ ! -f "$BUILD/rootfs/layout/index.json" ]; then
    log "rootfs: tar-out | split into layers"
    rm -rf "$BUILD/rootfs"; mkdir -p "$BUILD/rootfs"
    groups=() excludes=()
    while IFS= read -r g; do groups+=(--group "$g"); done < <(bench get "$ID" build.rootfs_layers \
        | python3 -c 'import json,sys; [print(k+"="+",".join(v)) for k,v in json.load(sys.stdin).items()]')
    while IFS= read -r e; do excludes+=(--exclude "$e"); done < <(bench get "$ID" build.rootfs_excludes \
        | python3 -c 'import json,sys; [print(e) for e in json.load(sys.stdin)]')
    # tar-out streams into a fifo that split-tar reads, so the whole tree
    # never lands on disk uncompressed.
    guestfs bash -euo pipefail -c "
        mkfifo /tmp/rootfs.tar
        python3 /repo/scripts/bench-images/bench.py split-tar /tmp/rootfs.tar /work/rootfs/layers ${groups[*]} ${excludes[*]} >/work/rootfs/layers.json &
        split=\$!
        guestfish --ro -a /work/base-fixed.qcow2 -i tar-out / /tmp/rootfs.tar numericowner:true xattrs:true
        wait \$split
    "
    # The base image: the upstream tree only (no config beyond the arch);
    # container/Dockerfile adds the init, users' env and labels.
    printf '{"config":{"Env":["PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"]}}' >"$BUILD/rootfs/config.json"
    metas=(); for m in "$BUILD"/rootfs/layers/*.tar.gz.json; do metas+=(--layer "$m"); done
    bench layout "$BUILD/rootfs/layout" --platform "linux/$ARCH" --config "$BUILD/rootfs/config.json" "${metas[@]}" >/dev/null
    du -sh "$BUILD/rootfs/layers"
fi

if want container; then
    # The container variant: the upstream tree's layers plus an overlay
    # assembled from files (libs/images/bench/ID/container/overlay.py), as an
    # OCI layout. No docker build: nothing runs in the guest tree, and the
    # tree never has to be unpacked into a docker store.
    log "container overlay -> $WORK/out/$ARCH/rootfs-layout"
    over="$BUILD/container"
    rm -rf "$over" "$WORK/out/$ARCH/rootfs-layout"; mkdir -p "$over" "$WORK/out/$ARCH"
    manifest_for rootfs "$over/manifest.json"
    python3 "$REPO/libs/images/bench/$ID/container/overlay.py" "$WORK/sources.json" "$SPACESD" "$over" >/dev/null
    metas=(); for m in "$BUILD"/rootfs/layers/*.tar.gz.json; do metas+=(--layer "$m"); done
    metas+=(--layer "$over/overlay.tar.gz.json")
    bench layout "$WORK/out/$ARCH/rootfs-layout" --platform "linux/$ARCH" --config "$over/config.json" "${metas[@]}"
fi
log "done ($STAGES)"

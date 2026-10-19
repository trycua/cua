#!/usr/bin/env bash
# Build a libs/images image in both output forms from ONE definition.
#
#   rootfs         <repo>:docker-<tag>-<arch>   OCI rootfs for docker / gVisor (runsc)
#   disk           <out>/<arch>/disk.img        bootable GPT qcow2 (BIOS+UEFI amd64, UEFI arm64)
#   containerdisk  <repo>:<tag>-<arch>          KubeVirt containerDisk (/disk/disk.img, uid 107),
#                                               packed by the cua SDK's cua-image crate
#                                               (tools/cua-image-pack; CUA_IMAGE_PACKER=docker
#                                               uses common/containerdisk.Dockerfile instead)
#
# The rootfs is the source of truth. The disk is derived from it by layering
# common/vm/Dockerfile (kernel, systemd, cloud-init, netplan, sshd) on top,
# exporting that tree with buildx's tar exporter and piping it straight into
# the unprivileged disk builder (common/disk-builder), so the same guest ships
# in both forms. Tag convention matches trycua/cloud: `docker-` prefix = rootfs.
#
# Usage:
#   libs/images/build.sh <image> [--platform linux/arm64,linux/amd64] [--tag local]
#       [--repo ghcr.io/trycua/<name>] [--outputs rootfs,disk,containerdisk]
#       [--build-arg K=V]... [--build-context NAME=PATH]... [--label K=V]...
#       [--target STAGE] [--push] [--out DIR] [--disk-size 20G]
#   libs/images/build.sh manifest <repo> <tag> [--platform ...]   # join -<arch> tags
#
#   <image> is a directory under libs/images: linux, omarchy,
#   plain/ubuntu-xfce-vnc, plain/ubuntu-server, bench/<id>.
#   --label adds (or overrides) rootfs config labels; --build-context passes
#   a named buildx context through (bench images use both).
#   --target builds one Dockerfile stage (a tier of linux: slim, full; default
#   the last stage, full); its disk goes to <out>/<name>-<target>/<arch>
#   (`cua images build --target` does the same).
#
# Requires docker with buildx on the default docker driver (Colima / Docker
# Desktop / GitHub runners), so the vm layer can FROM the freshly loaded rootfs.
# Foreign-arch builds use binfmt (qemu-user / Rosetta).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

host_arch() { case "$(uname -m)" in arm64|aarch64) echo arm64 ;; *) echo amd64 ;; esac; }
log() { echo "[build $(date +%T)] $*" >&2; }

if [ "${1:-}" = manifest ]; then
    repo="${2:?repo}"; tag="${3:?tag}"; shift 3
    platforms="linux/amd64,linux/arm64"
    [ "${1:-}" = --platform ] && platforms="$2"
    for prefix in "docker-" ""; do
        srcs=()
        for p in ${platforms//,/ }; do srcs+=("$repo:${prefix}${tag}-${p#linux/}"); done
        log "manifest $repo:${prefix}${tag} <- ${srcs[*]}"
        docker buildx imagetools create -t "$repo:${prefix}${tag}" "${srcs[@]}"
    done
    exit 0
fi

IMAGE="${1:?image dir, e.g. linux}"; shift
# Pre-rename name, accepted for one release.
if [ "$IMAGE" = cua-desktop-linux ]; then
    echo "build.sh: cua-desktop-linux is now libs/images/linux; building linux" >&2
    IMAGE=linux
fi
IMAGE_DIR="$HERE/$IMAGE"
[ -f "$IMAGE_DIR/Dockerfile" ] || { echo "no Dockerfile in $IMAGE_DIR" >&2; exit 1; }
NAME="$(basename "$IMAGE")"
PLATFORMS="linux/$(host_arch)"
TAG=local
REPO="cua-e2e-local/$NAME"
OUTPUTS=rootfs
PUSH=0
OUT="${CUA_IMAGES_OUT:-$HOME/.cache/cua-images}"
DISK_SIZE="${DISK_SIZE:-20G}"
TARGET=""
BUILD_ARGS=()
EXTRA_LABELS=()
while [ $# -gt 0 ]; do
    case "$1" in
        --platform) PLATFORMS="$2"; shift 2 ;;
        --tag) TAG="$2"; shift 2 ;;
        --repo) REPO="$2"; shift 2 ;;
        --outputs) OUTPUTS="$2"; shift 2 ;;
        --build-arg) BUILD_ARGS+=(--build-arg "$2"); shift 2 ;;
        --build-context) BUILD_ARGS+=(--build-context "$2"); shift 2 ;;
        --label) EXTRA_LABELS+=(--label "$2"); shift 2 ;;
        --push) PUSH=1; shift ;;
        --out) OUT="$2"; shift 2 ;;
        --disk-size) DISK_SIZE="$2"; shift 2 ;;
        --target) TARGET="$2"; shift 2 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
want() { [[ ",$OUTPUTS," == *",$1,"* ]]; }
OUT_NAME="$NAME"
if [ -n "$TARGET" ]; then
    [[ "$TARGET" =~ ^[A-Za-z0-9._-]+$ ]] || { echo "--target $TARGET: expected a Dockerfile stage name" >&2; exit 2; }
    BUILD_ARGS+=(--target "$TARGET")
    OUT_NAME="$NAME-$TARGET"
fi
# Image metadata the cua resolver reads (ai.cua.image.os, ai.cua.spacesd; ai.cua.env-driver
# is the pre-rename label, still written for older SDKs):
# cua-spacesd ships in linux unless built with
# CUA_SPACESD_SOURCE=none; the plain images never carry it.
# An image carries cua-spacesd when its image.json lists a cua-spacesd service.
SPACESD=false
if [ -f "$IMAGE_DIR/image.json" ] && python3 -c 'import json,sys;sys.exit(0 if any(s.get("component")=="cua-spacesd" for s in json.load(open(sys.argv[1])).get("services",[])) else 1)' "$IMAGE_DIR/image.json"; then
    SPACESD=true
    for a in ${BUILD_ARGS[@]+"${BUILD_ARGS[@]}"}; do
        [ "$a" = CUA_SPACESD_SOURCE=none ] && SPACESD=false
    done
fi
META_LABELS=(--label "ai.cua.image.os=linux" --label "ai.cua.spacesd=$SPACESD" --label "ai.cua.env-driver=$SPACESD")
# Images with a manifest record the source revision they were built from.
SOURCE_REVISION="$(git -C "$HERE" rev-parse HEAD 2>/dev/null || true)"
[ -f "$IMAGE_DIR/image.json" ] && BUILD_ARGS+=(--build-arg "CUA_IMAGE_SOURCE_REVISION=$SOURCE_REVISION")
# The VM layer: common/vm/Dockerfile (Ubuntu), or the image's own vm.Dockerfile
# (e.g. omarchy, which is Arch).
VM_DOCKERFILE="$HERE/common/vm/Dockerfile"
[ -f "$IMAGE_DIR/vm.Dockerfile" ] && VM_DOCKERFILE="$IMAGE_DIR/vm.Dockerfile"
# containerDisk packer: `sdk` (default) = tools/cua-image-pack on the cua SDK's
# cua-image crate; `docker` = common/containerdisk.Dockerfile. sdk needs cargo.
PACKER="${CUA_IMAGE_PACKER:-sdk}"
PACK_BIN="$HERE/tools/cua-image-pack/target/release/cua-image-pack"
if want containerdisk && [ "$PACKER" = sdk ] && [ ! -x "$PACK_BIN" ]; then
    if command -v cargo >/dev/null; then
        log "building tools/cua-image-pack"
        (cd "$HERE/tools/cua-image-pack" && CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" cargo build --release --quiet)
    else
        log "cargo not found; falling back to CUA_IMAGE_PACKER=docker"; PACKER=docker
    fi
fi
# containerdisk needs the disk.
want containerdisk && ! want disk && OUTPUTS="$OUTPUTS,disk"

timed() { local t0=$SECONDS; "$@"; log "  took $((SECONDS - t0))s: ${*:1:4}"; }

for platform in ${PLATFORMS//,/ }; do
    arch="${platform#linux/}"
    rootfs_ref="$REPO:docker-$TAG-$arch"
    log "== $NAME $platform"

    log "rootfs -> $rootfs_ref"
    timed docker buildx build --platform "$platform" --load \
        -f "$IMAGE_DIR/Dockerfile" ${BUILD_ARGS[@]+"${BUILD_ARGS[@]}"} \
        --label "org.opencontainers.image.source=https://github.com/trycua/cua" \
        --label "ai.cua.image.variant=rootfs" "${META_LABELS[@]}" ${EXTRA_LABELS[@]+"${EXTRA_LABELS[@]}"} \
        -t "$rootfs_ref" "$HERE"
    # The labels are derived from build inputs; the manifest from what was
    # actually built. They must agree (e.g. no ai.cua.spacesd=true on an image
    # built without cua-spacesd).
    if docker run --rm --entrypoint test "$rootfs_ref" -f /etc/cua-image/manifest.json 2>/dev/null; then
        manifest_tmp="$(mktemp)"
        docker run --rm --entrypoint cat "$rootfs_ref" /etc/cua-image/manifest.json >"$manifest_tmp"
        python3 "$HERE/common/tools/cua-image-manifest" check-labels --manifest "$manifest_tmp" \
            --labels "$(docker inspect -f '{{json .Config.Labels}}' "$rootfs_ref")"
        rm -f "$manifest_tmp"
    fi
    [ "$PUSH" = 1 ] && timed docker push "$rootfs_ref"

    if want disk; then
        disk_dir="$OUT/$OUT_NAME/$arch"
        mkdir -p "$disk_dir"
        builder_ref="cua-e2e-local/disk-builder:$arch"
        log "disk-builder -> $builder_ref"
        timed docker buildx build --platform "$platform" --load -t "$builder_ref" "$HERE/common/disk-builder"
        log "vm root (from $rootfs_ref) | make-disk -> $disk_dir/disk.img"
        t0=$SECONDS
        docker buildx build --platform "$platform" \
            -f "$VM_DOCKERFILE" --build-arg "BASE_IMAGE=$rootfs_ref" \
            --output type=tar,dest=- "$HERE" \
            | docker run --rm -i --platform "$platform" --memory=4g --memory-swap=4g \
                -e DISK_SIZE="$DISK_SIZE" -e QCOW2_COMPRESS="${QCOW2_COMPRESS:-1}" \
                -v "$disk_dir:/out" "$builder_ref" - /out/disk.img
        log "  took $((SECONDS - t0))s: vm root + disk"
    fi

    if want containerdisk; then
        cd_ref="$REPO:$TAG-$arch"
        disk="$OUT/$OUT_NAME/$arch/disk.img"
        if [ "$PACKER" = sdk ]; then
            # Dogfood the SDK: cua_image::containerdisk::pack -> OCI layout
            # (docker load) and/or a direct registry push.
            log "containerdisk (cua-image) -> $cd_ref"
            pack_dir="$OUT/$OUT_NAME/$arch/containerdisk"
            rm -rf "$pack_dir"
            push_args=()
            [ "$PUSH" = 1 ] && push_args=(--push "$cd_ref")
            t0=$SECONDS
            "$PACK_BIN" disk "$disk" "$arch" "$pack_dir/blobs-work" --oci-layout "$pack_dir/layout" "${push_args[@]+"${push_args[@]}"}"
            # docker's classic (non-containerd) image store loads only
            # docker-archive tars: add manifest.json next to index.json
            # (the containerd store keeps reading index.json).
            python3 - "$pack_dir/layout" <<'PY'
import json, os, sys
root = sys.argv[1]
blob = lambda d: os.path.join("blobs", *d.split(":", 1))
index = json.load(open(os.path.join(root, "index.json")))
out = []
for desc in index["manifests"]:
    m = json.load(open(os.path.join(root, blob(desc["digest"]))))
    out.append({"Config": blob(m["config"]["digest"]), "RepoTags": None,
                "Layers": [blob(l["digest"]) for l in m["layers"]]})
json.dump(out, open(os.path.join(root, "manifest.json"), "w"))
PY
            id="$(cd "$pack_dir/layout" && tar -cf - . | docker load -q | sed -n 's/^Loaded image ID: //p')"
            docker tag "$id" "$cd_ref"
            rm -rf "$pack_dir"
            log "  took $((SECONDS - t0))s: cua-image pack + load"
        else
            log "containerdisk (docker build) -> $cd_ref"
            timed docker buildx build --platform "$platform" --load \
                -f "$HERE/common/containerdisk.Dockerfile" \
                --label "ai.cua.image.variant=containerdisk" "${META_LABELS[@]}" \
                -t "$cd_ref" "$OUT/$OUT_NAME/$arch"
            [ "$PUSH" = 1 ] && timed docker push "$cd_ref"
        fi
    fi
done
log "done: $NAME [$OUTPUTS] for $PLATFORMS"

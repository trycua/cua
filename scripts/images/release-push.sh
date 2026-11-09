#!/usr/bin/env bash
# `cua images release` push step: push one arch's doctored rootfs and disk as
# dated, immutable children and record what was pushed.
#
#   release-push.sh --image-dir DIR --repo REPO --tag TAG --arch ARCH --artifacts DIR --evidence DIR
#       --out OUT --cua CUA [--target TIER] [--no-rootfs]
#
# Checks the artifacts are the bytes release-save.sh recorded, refuses tags
# that exist (check-tag-safety.sh), pushes <repo>:docker-<tag>-<arch> (unless
# --no-rootfs) and packs the disk as <repo>:<tag>-<arch> (cua images pack),
# then writes EVIDENCE/pushed-<arch>/pushed.json (the subjects doctor reports
# attach to, and what verify compares).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IMAGE_DIR="" REPO="" TAG="" ARCH="" ARTIFACTS="" EVIDENCE="" OUT="" CUA="cua" TARGET="" ROOTFS=1
while [ $# -gt 0 ]; do
    case "$1" in
        --image-dir) IMAGE_DIR="$2"; shift 2 ;;
        --repo) REPO="$2"; shift 2 ;;
        --tag) TAG="$2"; shift 2 ;;
        --arch) ARCH="$2"; shift 2 ;;
        --artifacts) ARTIFACTS="$2"; shift 2 ;;
        --evidence) EVIDENCE="$2"; shift 2 ;;
        --out) OUT="$2"; shift 2 ;;
        --cua) CUA="$2"; shift 2 ;;
        --target) TARGET="$2"; shift 2 ;;
        --no-rootfs) ROOTFS=0; shift ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
[ -n "$IMAGE_DIR" ] && [ -n "$REPO" ] && [ -n "$TAG" ] && [ -n "$ARCH" ] && [ -n "$ARTIFACTS" ] && [ -n "$EVIDENCE" ] && [ -n "$OUT" ] \
    || { echo "--image-dir, --repo, --tag, --arch, --artifacts, --evidence and --out are required" >&2; exit 2; }
dir="$ARTIFACTS/$ARCH"
(cd "$dir" && sha256sum -c "$EVIDENCE/artifacts-$ARCH.sha256")
rootfs="$REPO:docker-$TAG-$ARCH" disk="$REPO:$TAG-$ARCH"
refs=("$disk"); [ "$ROOTFS" = 1 ] && refs+=("$rootfs")
"$HERE/check-tag-safety.sh" "${refs[@]}"
rootfs_digest=""
if [ "$ROOTFS" = 1 ]; then
    if command -v crane >/dev/null; then
        # The saved tarball as one single-arch manifest (docker push from a
        # containerd image store would push an index with attestations).
        tar="$(mktemp "${TMPDIR:-/tmp}/cua-e2e-rootfs.XXXXXX")"
        zstd -dc "$dir/rootfs/rootfs.tar.zst" >"$tar"
        pushed="$(crane push "$tar" "$rootfs")"; rm -f "$tar"
        rootfs_digest="${pushed##*@}"
    else
        "$HERE/load-image-artifact.sh" "$rootfs" "$dir/rootfs/rootfs.tar.zst"
        docker push "$rootfs"
        rootfs_digest="$(docker buildx imagetools inspect --format '{{json .Manifest}}' "$rootfs" | python3 -c 'import json,sys;print(json.load(sys.stdin)["digest"])')"
    fi
    echo "pushed $rootfs@$rootfs_digest"
fi
name="$(basename "$IMAGE_DIR")"; [ -n "$TARGET" ] && name="$name-$TARGET"
mkdir -p "$OUT/$name/$ARCH"
rm -f "$OUT/$name/$ARCH/disk.img"
ln "$dir/disk/disk.img" "$OUT/$name/$ARCH/disk.img" 2>/dev/null || cp "$dir/disk/disk.img" "$OUT/$name/$ARCH/disk.img"
target=(); [ -n "$TARGET" ] && target=(--target "$TARGET")
"$CUA" --json images pack "$IMAGE_DIR" ${target[@]+"${target[@]}"} --repo "$REPO" --platform "linux/$ARCH" --tag "$TAG" --out "$OUT" --push \
    | tee "$EVIDENCE/pack-$ARCH.json"
disk_digest="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["containerdisks"][0]["digest"])' "$EVIDENCE/pack-$ARCH.json")"
mkdir -p "$EVIDENCE/pushed-$ARCH"
python3 - "$REPO" "$ARCH" "$rootfs_digest" "$disk_digest" "$EVIDENCE/pushed-$ARCH/pushed.json" <<'PY'
import json, sys
repo, arch, rootfs, disk, out = sys.argv[1:6]
record = {"repo": repo, "arch": arch, "containerdisk": disk}
if rootfs:
    record["rootfs"] = rootfs
json.dump(record, open(out, "w"))
print(json.dumps(record))
PY

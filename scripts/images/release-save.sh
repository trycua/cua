#!/usr/bin/env bash
# `cua images release` stage step: keep the exact bytes the doctor checked.
#
#   release-save.sh --arch ARCH --disk DISK.img --artifacts DIR --evidence DIR [--rootfs REF]
#
# DIR/<arch>/disk/disk.img (hardlinked or copied, so a resumed run still has
# the build output) and, with --rootfs, DIR/<arch>/rootfs/rootfs.tar.zst.
# Their sha256 go to EVIDENCE/artifacts-<arch>.sha256 and the disk's alone to
# EVIDENCE/disk-<arch>.sha256; every later step checks it handles these bytes.
set -euo pipefail
ARCH="" DISK="" ARTIFACTS="" EVIDENCE="" ROOTFS=""
while [ $# -gt 0 ]; do
    case "$1" in
        --arch) ARCH="$2"; shift 2 ;;
        --disk) DISK="$2"; shift 2 ;;
        --artifacts) ARTIFACTS="$2"; shift 2 ;;
        --evidence) EVIDENCE="$2"; shift 2 ;;
        --rootfs) ROOTFS="$2"; shift 2 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
[ -n "$ARCH" ] && [ -f "$DISK" ] && [ -n "$ARTIFACTS" ] && [ -n "$EVIDENCE" ] || { echo "usage: see the header" >&2; exit 2; }
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
dir="$ARTIFACTS/$ARCH"
mkdir -p "$dir/disk" "$EVIDENCE"
rm -f "$dir/disk/disk.img"
ln "$DISK" "$dir/disk/disk.img" 2>/dev/null || cp "$DISK" "$dir/disk/disk.img"
files=(disk/disk.img)
if [ -n "$ROOTFS" ]; then
    "$HERE/load-image-artifact.sh" --save "$ROOTFS" "$dir/rootfs/rootfs.tar.zst"
    files=(rootfs/rootfs.tar.zst disk/disk.img)
fi
(cd "$dir" && sha256sum "${files[@]}") | tee "$EVIDENCE/artifacts-$ARCH.sha256"
(cd "$dir" && sha256sum disk/disk.img) | tee "$EVIDENCE/disk-$ARCH.sha256"

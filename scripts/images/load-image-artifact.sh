#!/usr/bin/env bash
# Make REF available to docker: load it from a `save-image-artifact` tarball
# of the calling workflow run when one is given, else pull it.
#
#   load-image-artifact.sh REF [ROOTFS.tar.zst]
#   load-image-artifact.sh --save REF OUT.tar.zst
#
# cd-image-linux.yml gates unpublished builds this way: the build job saves
# the rootfs it doctored, and image-doctor.yml's lanes load exactly those
# bytes instead of pulling a pushed tag.
set -euo pipefail
if [ "${1:-}" = --save ]; then
    ref="${2:?ref}"; out="${3:?out.tar.zst}"
    mkdir -p "$(dirname "$out")"
    docker save "$ref" | zstd -T0 -3 -q -o "$out"
    ls -la "$out"
    exit 0
fi
ref="${1:?ref}"; tarball="${2:-}"
if [ -n "$tarball" ]; then
    zstd -dc "$tarball" | docker load
    docker image inspect "$ref" >/dev/null || { echo "$tarball does not hold $ref" >&2; exit 1; }
else
    docker pull "$ref"
fi

#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
# Build the cua-relay image twice (the second time without cache) and check
# that the image digest and the binary sha256 are identical. Needs Docker
# with BuildKit >= 0.13 (OCI export with rewrite-timestamp).
#
#   libs/cua-spacesd/crates/cua-relay/scripts/check-reproducible.sh [platform]
#
# SOURCE_DATE_EPOCH defaults to the HEAD commit time; export it to rebuild
# another commit's image the way it was built.
set -euo pipefail

root=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
platform=${1:-linux/amd64}
export SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(git -C "$root" log -1 --format=%ct)}
dockerfile=libs/cua-spacesd/crates/cua-relay/Dockerfile
work=$(mktemp -d "${CHECK_REPRO_TMP:-${TMPDIR:-/tmp}}/relay-repro.XXXXXX")
trap 'rm -rf "$work"' EXIT
cd "$root"

oci_digest() { # manifest digest of the single-image OCI layout tar
  tar -xOf "$1" index.json | python3 -c 'import json,sys; print(json.load(sys.stdin)["manifests"][0]["digest"])'
}

build() { # name, hosted (0|1), extra flags...
  local n=$1 h=$2; shift 2
  docker buildx build "$@" --platform "$platform" -f "$dockerfile" \
    --build-arg SOURCE_DATE_EPOCH="$SOURCE_DATE_EPOCH" --build-arg CUA_RELAY_HOSTED="$h" \
    --output "type=oci,dest=$work/$n.tar,rewrite-timestamp=true" libs
  docker buildx build "$@" --platform "$platform" -f "$dockerfile" \
    --build-arg SOURCE_DATE_EPOCH="$SOURCE_DATE_EPOCH" --build-arg CUA_RELAY_HOSTED="$h" --target binary \
    --output "type=local,dest=$work/$n-bin,rewrite-timestamp=true" libs
  eval "digest_$n=$(oci_digest "$work/$n.tar")"
  eval "bin_$n=$(shasum -a 256 "$work/$n-bin/cua-relay" | cut -d' ' -f1)"
  echo "$n (CUA_RELAY_HOSTED=$h): image $platform $(eval echo "\$digest_$n")  binary $(eval echo "\$bin_$n")"
}

echo "SOURCE_DATE_EPOCH=$SOURCE_DATE_EPOCH"
fail=0
# Both variants: the default (self-hosted, Free) build and the hosted build.
for h in 0 1; do
  build "one$h" "$h"
  build "two$h" "$h" --no-cache
  eval "d1=\$digest_one$h d2=\$digest_two$h b1=\$bin_one$h b2=\$bin_two$h"
  [ "$d1" = "$d2" ] || { echo "CUA_RELAY_HOSTED=$h: image digests differ" >&2; fail=1; }
  [ "$b1" = "$b2" ] || { echo "CUA_RELAY_HOSTED=$h: binary hashes differ" >&2; fail=1; }
  [ "$fail" = 0 ] && echo "reproducible (CUA_RELAY_HOSTED=$h): image $d1 binary $b1"
done
exit $fail

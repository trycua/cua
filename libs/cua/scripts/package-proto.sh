#!/usr/bin/env bash
# Build cua-proto-<version>.tar.gz, the release asset with the protobuf
# contract for other languages: the .proto files, buf.yaml, buf.gen.yaml and
# a README with grpcurl and `buf generate` examples (proto/RELEASE-README.md).
#
# Usage: libs/cua/scripts/package-proto.sh <version> <out-dir>
set -euo pipefail

version="${1:?usage: package-proto.sh <version> <out-dir>}"
out="${2:?usage: package-proto.sh <version> <out-dir>}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
src="$here/../proto"
name="cua-proto-$version"

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/$name" "$out"
cp "$src/buf.yaml" "$src/buf.gen.yaml" "$stage/$name/"
cp "$src/RELEASE-README.md" "$stage/$name/README.md"
(cd "$src" && find cua -name '*.proto' | LC_ALL=C sort) | while read -r f; do
  mkdir -p "$stage/$name/$(dirname "$f")"
  cp "$src/$f" "$stage/$name/$f"
done

# Reproducible: fixed order, owner and mtime (the commit time when in git).
mtime="${SOURCE_DATE_EPOCH:-$(git -C "$src" log -1 --format=%ct 2>/dev/null || echo 0)}"
find "$stage/$name" -exec touch -h -d "@$mtime" {} + 2>/dev/null \
  || find "$stage/$name" -exec touch -h -t "$(date -u -r "$mtime" +%Y%m%d%H%M.%S)" {} +
tarball="$(cd "$out" && pwd)/$name.tar.gz"
(cd "$stage" && find "$name" -print | LC_ALL=C sort \
  | tar -cf - --no-recursion -T - --owner=0 --group=0 --numeric-owner) \
  | gzip -n > "$tarball"
echo "$tarball"

#!/usr/bin/env bash
# Copy a single-platform image into <dst-repo> as an OCI image manifest and
# print its digest (untagged, content-addressed).
#
#   scripts/images/to-oci-manifest.sh <src-ref> <dst-repo>
#
# `docker push` (classic image store, GitHub runners) writes Docker v2
# manifests, and a Docker manifest list cannot carry annotations, so
# `buildx imagetools create --annotation index:...` would silently drop them.
# This rewrites only the media types (the config and layer blobs are the same
# bytes), so the canonical indexes can be OCI indexes with annotations.
# Needs crane and oras, with registry credentials in the docker config.
set -euo pipefail
src="${1:?src ref}" dst="${2:?dst repo}"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT

crane manifest "$src" >"$tmp/src.json"
mt="$(jq -r .mediaType "$tmp/src.json")"
src_digest="$(crane digest "$src")"
# Blobs (and the source manifest, untagged) into the destination repo.
crane copy "$src" "$dst@$src_digest" >&2
case "$mt" in
    application/vnd.oci.image.manifest.v1+json) echo "$src_digest"; exit 0 ;;
    application/vnd.docker.distribution.manifest.v2+json) ;;
    *) echo "unsupported manifest media type $mt for $src" >&2; exit 2 ;;
esac
jq -c '
  .mediaType = "application/vnd.oci.image.manifest.v1+json"
  | .config.mediaType = "application/vnd.oci.image.config.v1+json"
  | .layers |= map(.mediaType |= (
      if . == "application/vnd.docker.image.rootfs.diff.tar.gzip" then "application/vnd.oci.image.layer.v1.tar+gzip"
      elif . == "application/vnd.docker.image.rootfs.diff.tar" then "application/vnd.oci.image.layer.v1.tar"
      elif . == "application/vnd.docker.image.rootfs.diff.tar.zstd" then "application/vnd.oci.image.layer.v1.tar+zstd"
      else error("unsupported layer media type " + .) end))
' "$tmp/src.json" >"$tmp/oci.json"
digest="sha256:$( (sha256sum "$tmp/oci.json" 2>/dev/null || shasum -a 256 "$tmp/oci.json") | cut -d' ' -f1)"
oras manifest push --media-type application/vnd.oci.image.manifest.v1+json "$dst@$digest" "$tmp/oci.json" >&2
[ "$(crane digest "$dst@$digest")" = "$digest" ] || { echo "pushed digest mismatch" >&2; exit 1; }
echo "$digest"

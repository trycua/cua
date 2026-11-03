#!/usr/bin/env bash
# Push a built Windows disk.img as the containerDisk index
# `ghcr.io/trycua/windows:2022-disk-<stamp>` (new immutable tag; refused if it
# exists, scripts/images/check-tag-safety.sh). KubeVirt's containerDisk
# contract: FROM scratch, the qcow2 at /disk/disk.img owned by 107:107
# (libs/images/common/containerdisk.Dockerfile), linux/amd64.
#
#   push-disk.sh --disk-img FILE --stamp YYYYMMDD-sha7 --manifest FILE [--repo REPO]
#
# --manifest is the build's manifest.json (build-image.sh); its base and
# cua-spacesd build go into the annotations. Prints `disk_digest=<digest>`.
# Needs docker buildx, crane, oras, jq and push access (DOCKER_CONFIG or
# ~/.docker).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHECK="$HERE/../../../scripts/images/check-tag-safety.sh"
IMG="" STAMP="" MANIFEST="" REPO=ghcr.io/trycua/windows
while [ $# -gt 0 ]; do
    case "$1" in
        --disk-img) IMG="$2"; shift 2 ;;
        --stamp) STAMP="$2"; shift 2 ;;
        --manifest) MANIFEST="$2"; shift 2 ;;
        --repo) REPO="$2"; shift 2 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
[ -f "$IMG" ] && [ -f "$MANIFEST" ] || { echo "--disk-img and --manifest are required" >&2; exit 2; }
[[ "$STAMP" =~ ^[0-9]{8}-[0-9a-f]{7}$ ]] || { echo "bad stamp $STAMP" >&2; exit 2; }
for t in crane oras jq; do command -v "$t" >/dev/null || { echo "missing $t" >&2; exit 2; }; done
ORAS_CFG=()
[ -n "${DOCKER_CONFIG:-}" ] && ORAS_CFG=(--registry-config "$DOCKER_CONFIG/config.json")
PIN="$REPO:2022-disk-$STAMP" CHILD="$REPO:2022-disk-$STAMP-amd64"
INDEX_MT=application/vnd.oci.image.index.v1+json
"$CHECK" "$PIN" "$CHILD"

WORK="$(mktemp -d "$(dirname "$IMG")/cua-e2e-pushdisk.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/ctx"
ln "$IMG" "$WORK/ctx/disk.img" 2>/dev/null || cp "$IMG" "$WORK/ctx/disk.img"

echo "==> pushing the amd64 manifest ($CHILD)"
docker buildx build --platform linux/amd64 --provenance=false --sbom=false \
    -f "$HERE/../common/containerdisk.Dockerfile" \
    --output "type=image,name=$CHILD,push=true,oci-mediatypes=true" "$WORK/ctx"
rm -rf "$WORK/ctx"
child_digest="$(crane digest "$CHILD")"
child_size="$(crane manifest "$REPO@$child_digest" | wc -c | tr -d ' ')"
child_mt="$(crane manifest "$REPO@$child_digest" | jq -r .mediaType)"

base="$(jq -r '.build.base // ""' "$MANIFEST")"
version="$(jq -r '.spacesd.version // ""' "$MANIFEST")"
git_sha="$(jq -r '.source_revision // ""' "$MANIFEST")"
jq -n --arg d "$child_digest" --argjson s "$child_size" --arg mt "$child_mt" \
    --arg base "$base" --arg v "$version" --arg rev "$git_sha" '{
  schemaVersion: 2,
  mediaType: "application/vnd.oci.image.index.v1+json",
  manifests: [{mediaType: $mt, digest: $d, size: $s, platform: {architecture: "amd64", os: "linux"}}],
  annotations: {
    "org.opencontainers.image.source": "https://github.com/trycua/cua",
    "org.opencontainers.image.revision": $rev,
    "org.opencontainers.image.title": "windows-2022 containerDisk",
    "ai.cua.image.os": "windows",
    "ai.cua.image.variant": "containerdisk",
    "ai.cua.image.base": $base,
    "ai.cua.spacesd": "true",
    "ai.cua.env-driver": "true",
    "ai.cua.spacesd.version": $v
  }}' >"$WORK/index.json"
want="sha256:$(shasum -a 256 "$WORK/index.json" | cut -d' ' -f1)"
echo "==> pushing the index ($PIN)"
oras manifest push ${ORAS_CFG[@]+"${ORAS_CFG[@]}"} --media-type "$INDEX_MT" "$PIN" "$WORK/index.json" >/dev/null
[ "$(crane digest "$PIN")" = "$want" ] || { echo "$PIN is not $want after the push" >&2; exit 1; }
echo "child_digest=$child_digest"
echo "disk_digest=$want"

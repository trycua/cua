#!/usr/bin/env bash
# Pushes the disks build.sh wrote to ghcr.io/trycua/runtime-gvisor as OCI
# artifacts: one manifest per arch (<tag>-<arch>, a single gzip raw-disk
# layer) and an index (<tag>). Tags are written once and never moved (no
# floating tag: the SDK pins each arch's layer digest in cua-vmm's
# managed.rs), so check-tag-safety.sh refuses a re-push of a different disk.
#
#   libs/images/runtime-gvisor/push.sh 0.1.0-rc1 out/
#
# Needs oras and a registry login (oras login ghcr.io, or REGISTRY_CONFIG).
# Prints each arch's layer digest and size for managed.rs.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
# shellcheck source=versions.env
. "$here/versions.env"

tag="${1:?usage: push.sh TAG OUT_DIR}"
out="${2:?usage: push.sh TAG OUT_DIR}"
repo="${REPO:-ghcr.io/trycua/runtime-gvisor}"
case "$tag" in
"$RUNTIME_VERSION" | "$RUNTIME_VERSION"-rc[0-9]*) ;;
*) echo "tag must be $RUNTIME_VERSION or $RUNTIME_VERSION-rcN (versions.env)" >&2; exit 2 ;;
esac
cfg=()
[ -n "${REGISTRY_CONFIG:-}" ] && cfg=(--registry-config "$REGISTRY_CONFIG")

children=()
for arch in arm64 amd64; do
    file="runtime-gvisor-$RUNTIME_VERSION-$arch.raw.gz"
    [ -f "$out/$file" ] || continue
    (cd "$out" && shasum -a 256 -c "$file.sha256" >/dev/null)
    "$root/scripts/images/check-tag-safety.sh" "$repo:$tag-$arch"
    (cd "$out" && oras push "${cfg[@]}" "$repo:$tag-$arch" \
        --artifact-type application/vnd.trycua.runtime.v1 \
        --annotation "org.opencontainers.image.source=https://github.com/trycua/cua" \
        --annotation "org.opencontainers.image.version=$tag" \
        --annotation "org.opencontainers.image.description=Cua built-in Linux runtime VM disk (Colima $COLIMA_VERSION, gVisor $GVISOR_VERSION)" \
        --annotation "ai.cua.runtime.arch=$arch" \
        --annotation "ai.cua.runtime.gvisor=$GVISOR_VERSION" \
        --annotation "ai.cua.runtime.colima=$COLIMA_VERSION" \
        "$file:application/vnd.trycua.runtime.disk.raw.v1+gzip" >/dev/null)
    children+=("$repo:$tag-$arch")
    digest="sha256:$(cut -d' ' -f1 "$out/$file.sha256")"
    size="$(wc -c < "$out/$file" | tr -d ' ')"
    echo "$arch layer $digest size $size"
done
[ ${#children[@]} -gt 0 ] || { echo "no disks in $out" >&2; exit 2; }
"$root/scripts/images/check-tag-safety.sh" "$repo:$tag"
oras manifest index create "${cfg[@]}" "$repo:$tag" "${children[@]}" \
    --annotation "org.opencontainers.image.source=https://github.com/trycua/cua" >/dev/null
echo "pushed $repo:$tag (${children[*]})"

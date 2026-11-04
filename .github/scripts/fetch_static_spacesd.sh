#!/usr/bin/env bash
# Downloads the static (musl) headless cua-spacesd the `cua` CLI and the
# Spaces app bundle to inject into OCI images, from the cua-spacesd release
# libs/cua-spacesd/VERSION pins, checks its SHA-256 and installs it as
# DEST/cua-spacesd-linux-<arch>, where the daemon looks for it (next to `cua`,
# or in an app's Contents/Resources).
#
# Usage: fetch_static_spacesd.sh REPO ARCH|LABEL DEST
#   ARCH   x86_64 or aarch64 (or both, comma separated)
#   LABEL  a cd-cua-sdk native label (darwin-arm64, linux-x64-gnu, ...): its
#          own architecture
# Needs `gh` (GH_TOKEN).
set -euo pipefail

REPO="$1" WHAT="$2" DEST="$3"
ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
VERSION="$(tr -d '[:space:]' < "$ROOT/libs/cua-spacesd/VERSION")"
TAG="cua-spacesd-v$VERSION"

case "$WHAT" in
  *,*) IFS=, read -r -a arches <<<"$WHAT" ;;
  *arm64* | *aarch64*) arches=(aarch64) ;;
  *x64* | *x86_64*) arches=(x86_64) ;;
  *) echo "cannot tell the architecture from $WHAT" >&2; exit 2 ;;
esac

mkdir -p "$DEST"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
# Some native jobs build inside a container without `gh`: fall back to the
# REST API with curl (GH_TOKEN, when set, also reads a private repository).
download() {
  local name="$1" f
  if command -v gh >/dev/null 2>&1; then
    gh release download "$TAG" --repo "$REPO" --dir "$tmp" --pattern "$name" --pattern "$name.sha256"
    return
  fi
  local auth=()
  [ -n "${GH_TOKEN:-}" ] && auth=(-H "Authorization: Bearer $GH_TOKEN")
  local rel
  rel="$(curl -fsSL "${auth[@]}" -H "Accept: application/vnd.github+json" "https://api.github.com/repos/$REPO/releases/tags/$TAG")" || return 1
  for f in "$name" "$name.sha256"; do
    local id
    id="$(python3 -c 'import json,sys; n=sys.argv[1]; print(next((a["id"] for a in json.load(sys.stdin)["assets"] if a["name"]==n), ""))' "$f" <<<"$rel")"
    [ -n "$id" ] || return 1
    curl -fsSL "${auth[@]}" -H "Accept: application/octet-stream" -o "$tmp/$f" "https://api.github.com/repos/$REPO/releases/assets/$id" || return 1
  done
}

for arch in "${arches[@]}"; do
  asset="cua-spacesd-$arch-unknown-linux-musl"
  download "$asset"
  want="$(awk '{print $1}' "$tmp/$asset.sha256")"
  if command -v sha256sum >/dev/null 2>&1; then
    got="$(sha256sum "$tmp/$asset" | awk '{print $1}')"
  else
    got="$(shasum -a 256 "$tmp/$asset" | awk '{print $1}')"
  fi
  [ "$want" = "$got" ] || { echo "$asset: SHA-256 $got does not match $want" >&2; exit 1; }
  install -m 0755 "$tmp/$asset" "$DEST/cua-spacesd-linux-$arch"
  echo "bundled $DEST/cua-spacesd-linux-$arch (cua-spacesd $VERSION)"
done

#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Packages one cua-spacesd binary into the release assets its consumers
# download from https://github.com/trycua/cua/releases/download/cua-spacesd-v<version>/:
#
#   cua-spacesd-<triple>.tar.gz   Linux/macOS: kasm, xfce, qemu-docker, lume,
#                                    cua-sandbox builder
#   cua-spacesd-<triple>.zip      Windows: qemu-docker, cua-sandbox builder
#   cua-spacesd-<os>-<arch>.tar.gz  cua-host (`cua host setup`),
#                                    libs/images/linux
#   cua-spacesd-<triple>[.exe]    bare binary: packaging/install.sh / install.ps1
#
# Every archive holds the binary (cua-spacesd or cua-spacesd.exe),
# LICENSE and THIRD_PARTY_NOTICES.md at its root, with no wrapper directory,
# so `tar -xz -C <bin> cua-spacesd` works. Each asset gets a
# `<asset>.sha256` holding `<hex>  <asset>` (the first token is the digest).
#
#   package.sh --binary PATH --target TRIPLE --out DIR
set -euo pipefail

binary="" target="" out=""
while [ $# -gt 0 ]; do
  case "$1" in
    --binary) binary="$2"; shift 2 ;;
    --target) target="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
[ -f "$binary" ] || { echo "--binary $binary does not exist" >&2; exit 2; }
[ -n "$target" ] && [ -n "$out" ] || { echo "pass --target and --out" >&2; exit 2; }

root="$(cd "$(dirname "$0")/../.." && pwd)"
arch="${target%%-*}"
case "$arch" in x86_64|aarch64) ;; *) echo "unsupported arch in $target" >&2; exit 2 ;; esac
case "$target" in
  *-unknown-linux-gnu) os=linux exe=cua-spacesd ;;
  *-apple-darwin) os=macos exe=cua-spacesd ;;
  *-pc-windows-msvc) os=windows exe=cua-spacesd.exe ;;
  *) echo "unsupported target $target" >&2; exit 2 ;;
esac

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi
}
write_sum() { # asset name inside $out
  local digest
  digest="$(cd "$out" && sha256 "$1" | awk '{print $1}')"
  printf '%s  %s\n' "$digest" "$1" >"$out/$1.sha256"
}

mkdir -p "$out"
out="$(cd "$out" && pwd)"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
install -m 0755 "$binary" "$stage/$exe"
install -m 0644 "$root/LICENSE" "$root/THIRD_PARTY_NOTICES.md" "$stage/"
members=("$exe" LICENSE THIRD_PARTY_NOTICES.md)

# No AppleDouble (._*) members from macOS bsdtar.
export COPYFILE_DISABLE=1
tarball() { tar -czf "$out/$1" -C "$stage" "${members[@]}"; }

assets=()
if [ "$os" = windows ]; then
  zip_name="cua-spacesd-$target.zip"
  rm -f "$out/$zip_name"
  if command -v zip >/dev/null 2>&1; then
    (cd "$stage" && zip -q -X "$out/$zip_name" "${members[@]}")
  elif command -v 7z >/dev/null 2>&1; then
    (cd "$stage" && 7z a -tzip -bd "$out/$zip_name" "${members[@]}" >/dev/null)
  else
    echo "neither zip nor 7z is available" >&2; exit 1
  fi
  assets+=("$zip_name")
  install -m 0755 "$binary" "$out/cua-spacesd-$target.exe"
  assets+=("cua-spacesd-$target.exe")
else
  tarball "cua-spacesd-$target.tar.gz"
  assets+=("cua-spacesd-$target.tar.gz")
  install -m 0755 "$binary" "$out/cua-spacesd-$target"
  assets+=("cua-spacesd-$target")
fi
tarball "cua-spacesd-$os-$arch.tar.gz"
assets+=("cua-spacesd-$os-$arch.tar.gz")

for asset in "${assets[@]}"; do
  write_sum "$asset"
  echo "$out/$asset"
done

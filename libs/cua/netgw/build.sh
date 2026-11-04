#!/usr/bin/env bash
# Builds cua-netgw, the Cua network gateway (see NOTICE), into <out-dir>.
#
#   libs/cua/netgw/build.sh [out-dir]               # this OS; on macOS a
#                                                   # universal binary, ad-hoc signed
#   GOOS=windows GOARCH=amd64 libs/cua/netgw/build.sh <out-dir>
#                                                   # cross-build (cua-netgw.exe)
#   CUA_NETGW_SIGN_IDENTITY="Developer ID Application: ..." \
#   CUA_NETGW_KEYCHAIN=<keychain> libs/cua/netgw/build.sh <out-dir>
#                                                   # macOS release: hardened runtime, timestamp
#
# CUA_NETGW_VERSION sets what `cua-netgw -version` prints. The runtime looks
# for cua-netgw next to `cua`, next to cua-vz, in ~/.cua/bin, then on PATH
# ($CUA_NETGW overrides).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
out="${1:-$here/bin}"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
version="${CUA_NETGW_VERSION:-dev}"
goos="${GOOS:-$(go env GOOS)}"
build() { # <goarch> <output>
  (cd "$here" && CGO_ENABLED=0 GOOS="$goos" GOARCH="$1" go build -trimpath \
    -ldflags "-s -w -X main.version=$version" -o "$2" ./cmd/cua-netgw)
}
if [ "$goos" = windows ]; then
  build "${GOARCH:-$(go env GOARCH)}" "$out/cua-netgw.exe"
  echo "$out/cua-netgw.exe"
  exit 0
fi
if [ "$goos" != darwin ]; then
  build "${GOARCH:-$(go env GOARCH)}" "$out/cua-netgw"
  echo "$out/cua-netgw"
  exit 0
fi
# macOS: one binary for Apple silicon and Intel Macs.
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
build arm64 "$tmp/arm64"
build amd64 "$tmp/amd64"
lipo -create "$tmp/arm64" "$tmp/amd64" -output "$out/cua-netgw"
identity="${CUA_NETGW_SIGN_IDENTITY:-}"
if [ -n "$identity" ]; then
  args=(--force --timestamp --options runtime --sign "$identity")
  if [ -n "${CUA_NETGW_KEYCHAIN:-}" ]; then args+=(--keychain "$CUA_NETGW_KEYCHAIN"); fi
else
  args=(--force --sign -)
fi
codesign "${args[@]}" --identifier ai.cua.netgw "$out/cua-netgw"
codesign --verify --strict "$out/cua-netgw"
echo "$out/cua-netgw"

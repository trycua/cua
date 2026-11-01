#!/usr/bin/env bash
# Builds cua-vz (the macOS microVM monitor) and signs it with its
# entitlement: Virtualization.framework refuses an unsigned VMM.
#
#   libs/cua/vz/build.sh [out-dir]          # ad-hoc signature (development)
#   CUA_VZ_SIGN_IDENTITY="Developer ID Application: ..." \
#   CUA_VZ_KEYCHAIN=<keychain> libs/cua/vz/build.sh <out-dir>
#                                           # release: hardened runtime, timestamp
#
# The runtime looks for cua-vz next to `cua`, in ~/.cua/bin, then on PATH
# ($CUA_VZ overrides).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
out="${1:-$here/.build/release}"
mkdir -p "$out"
swift build --package-path "$here" -c release --arch arm64 --product cua-vz
bin="$(swift build --package-path "$here" -c release --arch arm64 --show-bin-path)/cua-vz"
if [ "$(cd "$(dirname "$bin")" && pwd)/cua-vz" != "$(cd "$out" && pwd)/cua-vz" ]; then
  cp "$bin" "$out/cua-vz"
fi
identity="${CUA_VZ_SIGN_IDENTITY:-}"
if [ -n "$identity" ]; then
  args=(--force --timestamp --options runtime --sign "$identity")
  if [ -n "${CUA_VZ_KEYCHAIN:-}" ]; then args+=(--keychain "$CUA_VZ_KEYCHAIN"); fi
else
  args=(--force --sign -)
fi
codesign "${args[@]}" --identifier ai.cua.vz --entitlements "$here/cua-vz.entitlements" "$out/cua-vz"
codesign --verify --strict "$out/cua-vz"
codesign -d --entitlements - "$out/cua-vz" 2>/dev/null | grep -q com.apple.security.virtualization
echo "$out/cua-vz"

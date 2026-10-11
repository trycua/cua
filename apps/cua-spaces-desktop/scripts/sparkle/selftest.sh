#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Local proof, without the real Sparkle key or Developer ID: does Sparkle
# (accepts.sh, Sparkle's own checks) take a built Electron app as the
# update of the Swift app? The "old" app is a stand-in with the Swift app's
# Info.plist (bundle id, name, CFBundleVersion 0.7.2.900) and a throwaway
# Ed25519 key, ad hoc signed; the update is a disk image of the Electron
# app signed with that key. Then three controls that must be refused: the
# Electron app without SUPublicEDKey, a bad signature, and a downgrade.
#
#   pnpm dist:mac:dir && scripts/sparkle/selftest.sh "dist/mac-arm64/Cua Spaces.app"
#
# What it cannot show: the Developer ID team match (both apps ad hoc) and
# Gatekeeper on the installed app; see docs/sparkle-cutover.md.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
electron="${1:?usage: selftest.sh <Electron Cua Spaces.app>}"
swift_plist="$here/../../../cua-spaces-macos/Support/Info.plist"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
pb() { /usr/libexec/PlistBuddy "$@" >/dev/null; }
# One checkout of the pinned Sparkle for every run of accepts.sh.
if [ -z "${SPARKLE_SRC:-}" ]; then
  version="$(python3 -c 'import json,sys; print(next(p["state"]["version"] for p in json.load(open(sys.argv[1]))["pins"] if p["identity"] == "sparkle"))' "$here/../../../cua-spaces-macos/Package.resolved")"
  git clone -q --depth 1 --branch "$version" https://github.com/sparkle-project/Sparkle.git "$work/Sparkle" 2>/dev/null
  export SPARKLE_SRC="$work/Sparkle"
fi

# A throwaway key (CryptoKit): seed and public key, base64, and signing.
cat >"$work/ed.swift" <<'SWIFT'
import CryptoKit
import Foundation
let a = CommandLine.arguments
if a[1] == "gen" {
    let k = Curve25519.Signing.PrivateKey()
    print(k.rawRepresentation.base64EncodedString())
    print(k.publicKey.rawRepresentation.base64EncodedString())
} else {
    let k = try! Curve25519.Signing.PrivateKey(rawRepresentation: Data(base64Encoded: a[2])!)
    print(try! k.signature(for: FileManager.default.contents(atPath: a[3])!).base64EncodedString())
}
SWIFT
swiftc -O -o "$work/ed" "$work/ed.swift" 2>/dev/null
keys="$("$work/ed" gen)"
seed="$(sed -n 1p <<<"$keys")"
public="$(sed -n 2p <<<"$keys")"

# old_app VERSION -> the stand-in Swift app.
old_app() {
  local app="$work/old-$1/Cua Spaces.app"
  mkdir -p "$app/Contents/MacOS"
  cp "$swift_plist" "$app/Contents/Info.plist"
  pb -c "Set :CFBundleVersion $1" -c "Set :SUPublicEDKey $public" "$app/Contents/Info.plist"
  printf 'int main(void){return 0;}\n' >"$work/main.c"
  clang -o "$app/Contents/MacOS/$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$app/Contents/Info.plist")" "$work/main.c"
  codesign --force --sign - --identifier com.trycua.spaces.macos "$app" 2>/dev/null
  echo "$app"
}

# image NAME APP -> a disk image holding APP as "Cua Spaces.app".
image() {
  mkdir -p "$work/src-$1"
  ditto "$2" "$work/src-$1/Cua Spaces.app"
  ln -s /Applications "$work/src-$1/Applications"
  hdiutil create -quiet -fs HFS+ -format UDZO -volname "Cua Spaces" -srcfolder "$work/src-$1" "$work/$1.dmg"
  echo "$work/$1.dmg"
}

old="$(old_app 0.7.2.900)"
dmg="$(image electron "$electron")"
signature="$("$work/ed" sign "$seed" "$dmg")"
echo "== the Electron app as the Swift app's update"
"$here/accepts.sh" "$old" "$dmg" "$signature"

refused() { # refused WHAT OLD DMG SIGNATURE
  if "$here/accepts.sh" "$2" "$3" "$4" >"$work/out" 2>&1; then
    cat "$work/out"
    echo "FAIL control accepted: $1"
    exit 1
  fi
  echo "PASS refused: $1 ($(grep FAIL "$work/out" | head -n 1 | cut -c1-160))"
}
echo "== controls"
nokey="$work/nokey/Cua Spaces.app"
mkdir -p "$work/nokey"
ditto "$electron" "$nokey"
pb -c "Delete :SUPublicEDKey" "$nokey/Contents/Info.plist"
codesign --force --sign - "$nokey" 2>/dev/null
nokey_dmg="$(image nokey "$nokey")"
refused "no SUPublicEDKey in the new app" "$old" "$nokey_dmg" "$("$work/ed" sign "$seed" "$nokey_dmg")"
other="$("$work/ed" gen | sed -n 1p)"
refused "signed with another key" "$old" "$dmg" "$("$work/ed" sign "$other" "$dmg")"
refused "downgrade" "$(old_app 99.0.0.1)" "$dmg" "$signature"
echo "selftest passed"

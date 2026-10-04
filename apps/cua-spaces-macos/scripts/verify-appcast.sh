#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Checks one release's item in a Sparkle appcast the way the installed app
# will read it: the feed parses; the item for the disk image names its
# bundle version (sparkle:version is the app's CFBundleVersion, which
# Sparkle compares), the full version, the beta channel exactly for an
# X.Y.Z-suffix version, the app's minimum macOS and the image's exact
# length; and its EdDSA signature verifies with the public key the app
# ships (SUPublicEDKey, read from the app inside the image, and the one
# committed in Support/Info.plist). With CUA_SPACES_SPARKLE_ED_PRIVATE_KEY
# set, Sparkle's own sign_update --verify checks the signature too.
#
#   scripts/verify-appcast.sh --appcast cua-spaces-appcast.xml --dmg cua-spaces-0.2.0-darwin-universal.dmg \
#       --url https://.../cua-spaces-0.2.0-darwin-universal.dmg --version 0.2.0 [--sparkle-bin DIR]
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
appcast=""
dmg=""
url=""
version=""
bin="$here/.build/artifacts/sparkle/Sparkle/bin"
while [ $# -gt 0 ]; do
  case "$1" in
    --appcast) appcast="$2"; shift ;;
    --dmg) dmg="$2"; shift ;;
    --url) url="$2"; shift ;;
    --version) version="$2"; shift ;;
    --sparkle-bin) bin="$2"; shift ;;
    -h | --help) sed -n '5,19p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done
[ -f "$appcast" ] && [ -f "$dmg" ] && [ -n "$url" ] && [ -n "$version" ] ||
  { echo "usage: $0 --appcast FILE --dmg FILE --url URL --version X.Y.Z[-suffix]" >&2; exit 2; }

xmllint --noout "$appcast"

mnt="$(mktemp -d)"
trap 'hdiutil detach -quiet "$mnt" >/dev/null 2>&1; rmdir "$mnt" 2>/dev/null' EXIT
hdiutil attach -quiet -readonly -nobrowse -noautoopen -mountpoint "$mnt" "$dmg"
app="$(find "$mnt" -maxdepth 1 -name '*.app' | head -n 1)"
[ -n "$app" ] || { echo "no .app in $dmg" >&2; exit 1; }
plist="$app/Contents/Info.plist"
read_key() { /usr/libexec/PlistBuddy -c "Print :$1" "$2" 2>/dev/null; }
bundle_version="$(read_key CFBundleVersion "$plist")"
min_os="$(read_key LSMinimumSystemVersion "$plist")"
app_key="$(read_key SUPublicEDKey "$plist")"
committed_key="$(read_key SUPublicEDKey "$here/Support/Info.plist")"
[ -n "$app_key" ] || { echo "the app in $dmg has no SUPublicEDKey" >&2; exit 1; }
[ "$app_key" = "$committed_key" ] ||
  { echo "the app's SUPublicEDKey is not the one committed in Support/Info.plist" >&2; exit 1; }

# The item's fields, one per line: version, short version, channel,
# minimum macOS, length, signature.
fields="$(python3 - "$appcast" "$url" <<'PY'
import sys
import xml.etree.ElementTree as ET
path, url = sys.argv[1:]
SP = "{http://www.andymatuschak.org/xml-namespaces/sparkle}"
items = [i for i in ET.parse(path).iter("item")
         if i.find("enclosure") is not None and i.find("enclosure").get("url") == url]
if len(items) != 1:
    sys.exit(f"expected one item for {url}, found {len(items)}")
i = items[0]
e = i.find("enclosure")
text = lambda tag: (i.findtext(SP + tag) or "").strip()
for v in (text("version"), text("shortVersionString"), text("channel"), text("minimumSystemVersion"),
          e.get("length") or "", e.get(SP + "edSignature") or ""):
    print(v)
PY
)"
item_version="$(sed -n 1p <<<"$fields")"
item_short="$(sed -n 2p <<<"$fields")"
item_channel="$(sed -n 3p <<<"$fields")"
item_min="$(sed -n 4p <<<"$fields")"
item_length="$(sed -n 5p <<<"$fields")"
signature="$(sed -n 6p <<<"$fields")"
want_channel=""
[[ "$version" == *-* ]] && want_channel="beta"
fail=0
expect() { # expect WHAT GOT WANT
  if [ "$2" = "$3" ]; then echo "PASS $1 ($2)"; else echo "FAIL $1: got '$2', want '$3'"; fail=1; fi
}
expect "sparkle:version is the bundle version" "$item_version" "$bundle_version"
expect "sparkle:shortVersionString is the full version" "$item_short" "$version"
expect "channel" "$item_channel" "$want_channel"
expect "minimum macOS" "$item_min" "$min_os"
expect "length" "$item_length" "$(stat -f %z "$dmg")"
[ -n "$signature" ] && echo "PASS the enclosure has an EdDSA signature" || { echo "FAIL no sparkle:edSignature"; fail=1; }

# The app's check: Ed25519 over the image with SUPublicEDKey (CryptoKit).
checker="$(mktemp -d)"
cat >"$checker/verify.swift" <<'SWIFT'
import CryptoKit
import Foundation
let a = CommandLine.arguments
guard let key = Data(base64Encoded: a[1]), let signature = Data(base64Encoded: a[2]),
      let file = FileManager.default.contents(atPath: a[3]),
      let publicKey = try? Curve25519.Signing.PublicKey(rawRepresentation: key) else { exit(2) }
exit(publicKey.isValidSignature(signature, for: file) ? 0 : 1)
SWIFT
if swift "$checker/verify.swift" "$app_key" "$signature" "$dmg"; then
  echo "PASS the signature verifies with the app's SUPublicEDKey"
else
  echo "FAIL the signature does not verify with the app's SUPublicEDKey"
  fail=1
fi
rm -rf "$checker"
if [ -n "${CUA_SPACES_SPARKLE_ED_PRIVATE_KEY:-}" ]; then
  if printf '%s' "$CUA_SPACES_SPARKLE_ED_PRIVATE_KEY" |
    "$bin/sign_update" --verify --ed-key-file - "$dmg" "$signature" >/dev/null; then
    echo "PASS sign_update --verify"
  else
    echo "FAIL sign_update --verify"
    fail=1
  fi
fi
exit $fail

#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Packs a built (and, for a release, notarized and stapled) "Cua Spaces.app"
# into a compressed, read-only disk image with an Applications link, then
# signs the image. install.sh mounts it and copies the one .app it finds.
#
#   scripts/package-dmg.sh --app "path/Cua Spaces.app" --out cua-spaces-0.2.0-darwin-universal.dmg
#       [--sign IDENTITY]
#
# --sign takes the Developer ID Application identity; without it the image
# is left unsigned (a Developer ID image must also be notarized and stapled,
# which the release workflow does next).
set -euo pipefail
app=""
out=""
identity=""
while [ $# -gt 0 ]; do
  case "$1" in
    --app) app="$2"; shift ;;
    --out) out="$2"; shift ;;
    --sign) identity="$2"; shift ;;
    -h | --help) sed -n '5,14p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done
[ -d "$app" ] && [ -f "$app/Contents/Info.plist" ] || { echo "--app must be a .app bundle" >&2; exit 2; }
[ -n "$out" ] || { echo "--out is required" >&2; exit 2; }
case "$out" in *.dmg) ;; *) echo "--out must end in .dmg" >&2; exit 2 ;; esac

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
stage="$work/Cua Spaces"
mkdir -p "$stage"
ditto "$app" "$stage/$(basename "$app")"
ln -s /Applications "$stage/Applications"
mkdir -p "$(dirname "$out")"
rm -f "$out"
hdiutil create -quiet -volname "Cua Spaces" -srcfolder "$stage" -fs HFS+ \
  -format UDZO -imagekey zlib-level=9 -ov "$out"
if [ -n "$identity" ]; then
  codesign --force --timestamp --sign "$identity" "$out"
  codesign --verify --strict --verbose=2 "$out" >&2
fi
hdiutil verify -quiet "$out"
echo "$out"

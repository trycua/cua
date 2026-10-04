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

here="$(cd "$(dirname "$0")" && pwd)"
art="$here/../Support/dmg"
dmgbuild_version="1.6.7"
for f in background.png background@2x.png applications.icns; do
  [ -f "$art/$f" ] || { echo "missing $art/$f (scripts/dmg-art/render.sh draws it)" >&2; exit 1; }
done
[ -f "$app/Contents/Resources/AppIcon.icns" ] || { echo "$app has no Contents/Resources/AppIcon.icns" >&2; exit 2; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
python3 -m venv "$work/venv"
"$work/venv/bin/pip" install --quiet --disable-pip-version-check "dmgbuild==$dmgbuild_version"
swift "$here/make-alias.swift" /Applications "$work/Applications" "$art/applications.icns"
mkdir -p "$(dirname "$out")"
rm -f "$out"
"$work/venv/bin/dmgbuild" -s "$here/dmg-settings.py" \
  -D app="$app" -D alias="$work/Applications" -D art="$art" \
  "Cua Spaces" "$out" >&2
if [ -n "$identity" ]; then
  codesign --force --timestamp --sign "$identity" "$out"
  codesign --verify --strict --verbose=2 "$out" >&2
fi
hdiutil verify -quiet "$out"
echo "$out"

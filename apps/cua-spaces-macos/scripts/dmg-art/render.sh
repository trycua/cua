#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Renders art.html with headless Chrome into the checked-in assets:
#   Support/AppIcon.icns              the app icon (build-app.sh bundles it)
#   Support/dmg/background.png, @2x   the DMG window background
#   Support/dmg/applications.icns     the icon of the DMG's Applications alias
# Only needed when the art changes; releases use the committed files.
#
#   scripts/dmg-art/render.sh [--chrome PATH]
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
support="$here/../../Support"
chrome="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
while [ $# -gt 0 ]; do
  case "$1" in
    --chrome) chrome="$2"; shift ;;
    -h | --help) sed -n '5,11p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done
[ -x "$chrome" ] || { echo "Chrome not found at $chrome (pass --chrome)" >&2; exit 2; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# shot MODE WIDTH HEIGHT SCALE OUT
shot() {
  "$chrome" --headless=new --disable-gpu --hide-scrollbars --allow-file-access-from-files \
    --default-background-color=00000000 --force-device-scale-factor="$4" \
    --window-size="$2,$3" --virtual-time-budget=3000 \
    --screenshot="$5" "file://$here/art.html#$1" >/dev/null 2>&1
  [ -s "$5" ] || { echo "Chrome did not render #$1" >&2; exit 1; }
}

# icns NAME SOURCE_1024_PNG OUT
icns() {
  local set="$work/$1.iconset" s
  mkdir -p "$set"
  for s in 16 32 128 256 512; do
    sips -z "$s" "$s" "$2" --out "$set/icon_${s}x${s}.png" >/dev/null
    sips -z $((s * 2)) $((s * 2)) "$2" --out "$set/icon_${s}x${s}@2x.png" >/dev/null
  done
  iconutil -c icns "$set" -o "$3"
}

mkdir -p "$support/dmg"
# 2560x1440: the window shows the top-left 720x480, the rest is for resized windows
shot bg 2560 1440 1 "$support/dmg/background.png"
shot bg 2560 1440 2 "$support/dmg/background@2x.png"
shot icon 1024 1024 1 "$work/icon.png"
shot alias 1024 1024 1 "$work/alias.png"
icns app "$work/icon.png" "$support/AppIcon.icns"
icns alias "$work/alias.png" "$support/dmg/applications.icns"
ls -l "$support/AppIcon.icns" "$support/dmg"

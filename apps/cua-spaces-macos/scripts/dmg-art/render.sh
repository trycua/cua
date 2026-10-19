#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Renders art.html with headless Chrome into the checked-in assets:
#   Support/AppIcon.icns              the app icon (build-app.sh bundles it)
#   Support/dmg/background.png, @2x   the DMG window background
#   Support/dmg/applications.icns     the icon of the DMG's Applications alias
#   ../cua-spaces/src-tauri/icons/    app-icon.png (the 1024 px master) and the
#                                     Tauri / Electron / Linux / Windows set
#                                     (`tauri icon`, via npx), same icon
# Only needed when the art changes; releases use the committed files.
#
#   scripts/dmg-art/render.sh [--chrome PATH] [--bg-only]
#
# --bg-only redraws just the DMG background (art.html "bg"); the icons stay.
# Keep APP and APPS in art.html equal to icon_locations in dmg-settings.py.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
support="$here/../../Support"
tauri_icons="$here/../../../cua-spaces/src-tauri/icons"
chrome="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
bg_only=0
while [ $# -gt 0 ]; do
  case "$1" in
    --chrome) chrome="$2"; shift ;;
    --bg-only) bg_only=1 ;;
    -h | --help) sed -n '5,17p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done
[ -x "$chrome" ] || { echo "Chrome not found at $chrome (pass --chrome)" >&2; exit 2; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

shot() {
  "$chrome" --headless=new --disable-gpu --hide-scrollbars --allow-file-access-from-files \
    --default-background-color=00000000 --force-device-scale-factor="$4" \
    --window-size="$2,$3" --virtual-time-budget=3000 \
    --screenshot="$5" "file://$here/art.html#$1" >/dev/null 2>&1
  [ -s "$5" ] || { echo "Chrome did not render #$1" >&2; exit 1; }
}

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
shot bg 2560 1440 1 "$support/dmg/background.png"
shot bg 2560 1440 2 "$support/dmg/background@2x.png"
if [ "$bg_only" = 1 ]; then ls -l "$support/dmg"; exit 0; fi
shot icon 1024 1024 1 "$work/icon.png"
shot alias 1024 1024 1 "$work/alias.png"
icns app "$work/icon.png" "$support/AppIcon.icns"
icns alias "$work/alias.png" "$support/dmg/applications.icns"

# The other apps ship the same icon. Keep only the files the repo tracks
# (tauri icon also writes Android and iOS sets).
cp "$work/icon.png" "$tauri_icons/app-icon.png"
npx --yes @tauri-apps/cli@^2.5.0 icon "$tauri_icons/app-icon.png" -o "$work/tauri" >/dev/null
for f in 32x32.png 64x64.png 128x128.png 128x128@2x.png icon.png icon.icns icon.ico StoreLogo.png \
  Square30x30Logo.png Square44x44Logo.png Square71x71Logo.png Square89x89Logo.png Square107x107Logo.png \
  Square142x142Logo.png Square150x150Logo.png Square284x284Logo.png Square310x310Logo.png; do
  cp "$work/tauri/$f" "$tauri_icons/$f"
done
ls -l "$support/AppIcon.icns" "$support/dmg" "$tauri_icons"

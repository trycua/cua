#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Builds the checked-in assets. The DMG art is rendered from art.html with
# headless Chrome; the app icon is not drawn: it comes from the chosen PNGs
# src/app-icon-1024.png and src/app-icon-small-1024.png (app-icon.py):
#   Support/AppIcon.icns              the legacy (macOS 15 and older) app icon
#   Support/AppIcon.icon              NOT built here: the macOS 26 Icon Composer source
#                                     (background fill + layers); build-app.sh compiles it
#   Support/dmg/background.png, @2x   the DMG window background
#   Support/dmg/applications.icns     the icon of the DMG's Applications alias
#   ../cua-spaces/src-tauri/icons/    app-icon.png (the 1024 px master) and the
#                                     Tauri / Electron / Linux / Windows set
#                                     (app-icon.py), same icon
# Only needed when the art changes; releases use the committed files.
#
#   scripts/dmg-art/render.sh [--chrome PATH] [--bg-only | --app-icon-only]
#
# --bg-only redraws just the DMG background (art.html "bg"); the icons stay.
# --app-icon-only rebuilds just the app icon files (no Chrome, DMG art untouched).
# To change the app icon, replace the two PNGs in src/ and rerun.
# Keep APP and APPS in art.html equal to icon_locations in dmg-settings.py.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
support="$here/../../Support"
tauri_icons="$here/../../../cua-spaces/src-tauri/icons"
chrome="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
bg_only=0
icon_only=0
while [ $# -gt 0 ]; do
  case "$1" in
    --chrome) chrome="$2"; shift ;;
    --bg-only) bg_only=1 ;;
    --app-icon-only) icon_only=1 ;;
    -h | --help) sed -n '5,22p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done
[ "$icon_only" = 1 ] || [ -x "$chrome" ] || { echo "Chrome not found at $chrome (pass --chrome)" >&2; exit 2; }

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

if [ "$icon_only" = 0 ]; then
  mkdir -p "$support/dmg"
  shot bg 2560 1440 1 "$support/dmg/background.png"
  shot bg 2560 1440 2 "$support/dmg/background@2x.png"
  if [ "$bg_only" = 1 ]; then ls -l "$support/dmg"; exit 0; fi
  shot alias 1024 1024 1 "$work/alias.png"
  icns alias "$work/alias.png" "$support/dmg/applications.icns"
fi

# app-icon.py needs Pillow: install a pinned one into a throwaway venv.
python3 -m venv "$work/venv"
"$work/venv/bin/pip" install --quiet "pillow==11.3.0"
py="$work/venv/bin/python"

# The app icon: the chosen PNGs, with the larger-print small art at 16 to 64 px.
mkdir -p "$work/app.iconset"
"$py" "$here/app-icon.py" iconset "$work/app.iconset"
iconutil -c icns "$work/app.iconset" -o "$support/AppIcon.icns"

# The other apps ship the same icon. Keep only the files the repo tracks.
"$py" "$here/app-icon.py" master "$tauri_icons/app-icon.png"
"$py" "$here/app-icon.py" tauri "$tauri_icons"
cp "$support/AppIcon.icns" "$tauri_icons/icon.icns"
ls -l "$support/AppIcon.icns" "$support/dmg" "$tauri_icons"

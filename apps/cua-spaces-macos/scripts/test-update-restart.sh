#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# The upgrade test: install the PREVIOUS release, offer it the new build
# through a local appcast, and check what a user sees.
#
#   1. the previous app downloads the update by itself and its menu shows
#      "Version X available";
#   2. "Restart to update" (that menu entry) quits the app, and the app that
#      runs afterwards is the new version.
#
# Nothing here reaches the network except the optional download of the old
# image: the feed is served from 127.0.0.1, signed with a throwaway EdDSA key
# (put into both copies, which are re-signed ad hoc), and both copies get a
# test bundle id and CUA_HOME, so the real app, its settings and its updater
# state are never touched. It drives the menu with System Events, so the
# terminal needs Accessibility; without it the test says so and exits 77.
#
#   scripts/test-update-restart.sh --old-dmg OLD.dmg --new-dmg NEW.dmg [--legacy]
#
# --legacy is for a previous release known to have the dead button (0.20.0
# to 0.22.0): the test then expects the click to do nothing, and quitting
# (what a user does) to install the update.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
old_dmg="" new_dmg="" legacy=0 bin=""
while [ $# -gt 0 ]; do
  case "$1" in
    --old-dmg) old_dmg="$2"; shift ;;
    --new-dmg) new_dmg="$2"; shift ;;
    --legacy) legacy=1 ;;
    --sparkle-bin) bin="$2"; shift ;;
    -h | --help) sed -n '5,25p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done
[ -f "$old_dmg" ] && [ -f "$new_dmg" ] || { echo "--old-dmg and --new-dmg must be disk images" >&2; exit 2; }
if [ -z "$bin" ]; then
  bin="$here/.build/artifacts/sparkle/Sparkle/bin"
  [ -x "$bin/generate_appcast" ] || swift package --package-path "$here" resolve >/dev/null
fi
[ -x "$bin/generate_appcast" ] || { echo "no generate_appcast in $bin" >&2; exit 1; }

id="com.trycua.spaces.macos.updatetest"
work="$(mktemp -d "${HOME}/.cache/cua-update-test.XXXXXX" 2>/dev/null || { mkdir -p "$HOME/.cache"; mktemp -d "${HOME}/.cache/cua-update-test.XXXXXX"; })"
pids=()
mounts=()
cleanup() {
  for p in ${pids[@]+"${pids[@]}"}; do kill "$p" 2>/dev/null || true; done
  [ -n "${server:-}" ] && kill "$server" 2>/dev/null || true
  for m in ${mounts[@]+"${mounts[@]}"}; do hdiutil detach "$m" -quiet 2>/dev/null || true; done
  # The app's own children (daemon, helpers, Sparkle's installer) live in $work.
  pkill -f "$work" 2>/dev/null || true
  defaults delete "$id" >/dev/null 2>&1 || true
  rm -rf "$HOME/Library/Caches/$id" "$work"
}
trap cleanup EXIT
log() { echo "[update-test] $*"; }
fail() { echo "[update-test] FAIL: $*" >&2; exit 1; }
plist() { /usr/libexec/PlistBuddy -c "Print :$2" "$1/Contents/Info.plist"; }

# The throwaway key: a random seed, its public half from CryptoKit.
seed="$(head -c 32 /dev/urandom | base64)"
pub="$(printf '%s' "$seed" | swift -e '
import CryptoKit
import Foundation
let seed = Data(base64Encoded: readLine()!)!
print(try! Curve25519.Signing.PrivateKey(rawRepresentation: seed).publicKey.rawRepresentation.base64EncodedString())')"
[ -n "$pub" ] || fail "could not make a test key"

port="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
feed="http://127.0.0.1:$port/appcast.xml"

# prepare DMG OUT_APP_DIR: the app from a disk image, made a test copy.
prepare() {
  local dmg="$1" out="$2" mnt="$work/mnt-$(basename "$2")"
  mkdir -p "$mnt" "$out"
  hdiutil attach -nobrowse -readonly -mountpoint "$mnt" "$dmg" >/dev/null
  mounts+=("$mnt")
  ditto "$mnt/Cua Spaces.app" "$out/Cua Spaces.app"
  hdiutil detach "$mnt" -quiet; mounts=("${mounts[@]/$mnt}")
  xattr -cr "$out/Cua Spaces.app"
  local p="$out/Cua Spaces.app/Contents/Info.plist"
  /usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier $id" -c "Set :SUPublicEDKey $pub" -c "Set :SUFeedURL $feed" "$p"
  # --deep without the hardened runtime: an ad hoc app has no team, so its
  # (re-signed) frameworks must not be library-validated against one.
  codesign --force --deep --sign - --identifier "$id" "$out/Cua Spaces.app" 2>/dev/null
}
log "preparing the previous release and the new build"
prepare "$old_dmg" "$work/old"
prepare "$new_dmg" "$work/new"
old_app="$work/old/Cua Spaces.app"
old_version="$(plist "$old_app" CFBundleShortVersionString)"
new_version="$(plist "$work/new/Cua Spaces.app" CFBundleShortVersionString)"
new_build="$(plist "$work/new/Cua Spaces.app" CFBundleVersion)"
log "previous $old_version ($(plist "$old_app" CFBundleVersion)), new $new_version ($new_build)"

# The feed: the new app in a disk image, signed with the test key.
mkdir -p "$work/feed"
hdiutil create -quiet -volname "Cua Spaces" -srcfolder "$work/new" -format UDZO "$work/feed/cua-spaces-$new_version-darwin-universal.dmg"
printf '%s' "$seed" | "$bin/generate_appcast" --ed-key-file - --download-url-prefix "http://127.0.0.1:$port/" \
  -o "$work/feed/appcast.xml" "$work/feed" >/dev/null
grep -q "sparkle:edSignature" "$work/feed/appcast.xml" || fail "the test appcast has no signature"
(cd "$work/feed" && exec python3 -m http.server "$port" --bind 127.0.0.1 >"$work/server.log" 2>&1) &
server=$!

# Menu access: skip (77) when System Events may not drive menus.
menu_items() { # PID -> the app menu's item names, one per line
  osascript -e 'on run argv
    tell application "System Events"
      set p to first process whose unix id is (item 1 of argv as integer)
      set out to ""
      repeat with mi in (menu items of menu 1 of menu bar item 2 of menu bar 1 of p)
        try
          set out to out & (name of mi) & linefeed
        end try
      end repeat
      return out
    end tell
  end run' "$1" 2>"$work/osa.err"
}
click_item() { # PID NAME
  osascript -e 'on run argv
    tell application "System Events"
      set p to first process whose unix id is (item 1 of argv as integer)
      click menu item (item 2 of argv) of menu 1 of menu bar item 2 of menu bar 1 of p
    end tell
  end run' "$1" "$2" >/dev/null 2>"$work/osa.err"
}

log "starting the previous release"
export CUA_HOME="$work/cua-home"
mkdir -p "$CUA_HOME"
defaults delete "$id" >/dev/null 2>&1 || true
"$old_app/Contents/MacOS/CuaSpacesMac" >"$work/app.log" 2>&1 &
app=$!
pids+=("$app")

# 1. The silent download.
entry=""
for _ in $(seq 1 90); do
  kill -0 "$app" 2>/dev/null || { cat "$work/app.log" >&2; fail "the previous release exited"; }
  if items="$(menu_items "$app")"; then
    entry="$(printf '%s\n' "$items" | grep -E "^Version .* available$" | head -1 || true)"
    [ -n "$entry" ] && break
  elif grep -qiE "not allowed|assistive|1743|-25211" "$work/osa.err"; then
    log "SKIP: this terminal may not drive menus (Accessibility)"; cat "$work/osa.err" >&2; exit 77
  fi
  sleep 2
done
[ -n "$entry" ] || { cat "$work/osa.err" "$work/app.log" >&2; fail "no \"Version X available\" entry: the update was not downloaded"; }
log "downloaded in the background: \"$entry\""

# 2. Restart to update.
installed() { plist "$old_app" CFBundleShortVersionString; }
click_item "$app" "$entry" || { cat "$work/osa.err" >&2; fail "could not click \"$entry\""; }
if [ "$legacy" = 1 ]; then
  sleep 25
  kill -0 "$app" 2>/dev/null || fail "the click restarted the app (it is not a legacy release)"
  log "as expected for this release: the click did nothing"
  osascript -e 'tell application "System Events" to tell (first process whose unix id is '"$app"') to click menu item "Quit Cua Spaces" of menu 1 of menu bar item 2 of menu bar 1' 2>/dev/null || true
fi
for _ in $(seq 1 60); do
  [ "$(installed)" = "$new_version" ] && break
  sleep 2
done
[ "$(installed)" = "$new_version" ] || { cat "$work/app.log" >&2; fail "the app on disk is still $(installed), not $new_version"; }
log "installed: $new_version"
if [ "$legacy" = 0 ]; then
  # The app that runs now is the new one.
  running=""
  for _ in $(seq 1 30); do
    running="$(pgrep -f "$old_app/Contents/MacOS/CuaSpacesMac" | head -1 || true)"
    [ -n "$running" ] && break
    sleep 2
  done
  [ -n "$running" ] || fail "the new version is installed but not running"
  pids+=("$running")
  log "the new version is running (pid $running)"
fi
log "PASS"

#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Checks a downloaded Cua Spaces for macOS disk image the way Gatekeeper and
# the Keyvault will: the image and the app are Developer ID signed,
# notarized and stapled; the app, its bundled `cua`, the SDK library and the
# updater (Sparkle.framework, its XPC services, Autoupdate and Updater.app)
# carry the Cua team, the hardened runtime and the expected identifiers and
# entitlements; the updater's feed and key are set; and every binary has
# both architectures. It mounts the image read-only and installs nothing.
#
# Keyvault smoke: it then starts the bundled `cua daemon` with a throwaway
# CUA_HOME (under ~/.cache, deleted afterwards; never ~/.cua, no loopback
# port, telemetry off) and runs the bundled `cua keyvault status` against
# it. The client refuses a daemon that fails the Keyvault's production
# requirement ("not the Cua daemon"), and the daemon refuses a client that
# is not first party, so a status answer means both halves pass. It reads
# no keychain and creates no vault. --no-keyvault-smoke skips it.
#
#   scripts/verify-release.sh cua-spaces-0.2.0-darwin-universal.dmg [--team YCK386LBJ7]
#       [--arch universal|arm64|x86_64] [--no-keyvault-smoke]
#
# Exits non-zero when any check fails; prints every check either way.
set -uo pipefail
dmg=""
team="YCK386LBJ7"
arch=universal
smoke=1
while [ $# -gt 0 ]; do
  case "$1" in
    --team) team="$2"; shift ;;
    --arch) arch="$2"; shift ;;
    --no-keyvault-smoke) smoke=0 ;;
    -h | --help) sed -n '5,24p' "$0"; exit 0 ;;
    -*) echo "unknown option: $1" >&2; exit 2 ;;
    *) dmg="$1" ;;
  esac
  shift
done
[ -f "$dmg" ] || { echo "usage: $0 <path/to.dmg> [--team TEAMID]" >&2; exit 2; }
case "$arch" in universal) archs="arm64 x86_64" ;; arm64 | x86_64) archs="$arch" ;; *) echo "bad --arch" >&2; exit 2 ;; esac

failed=0
section() { printf '\n== %s\n' "$1"; }
check() { # check NAME CMD...: run CMD, show its output, record the result
  local name="$1"; shift
  local output
  output="$("$@" 2>&1)"
  local rc=$?
  [ -n "$output" ] && printf '%s\n' "$output" | sed 's/^/   /'
  if [ $rc -eq 0 ]; then echo "PASS $name"; else echo "FAIL $name"; failed=1; fi
}

mnt="$(mktemp -d)"
kv_home=""
daemon_pid=""
cleanup() {
  [ -n "$daemon_pid" ] && kill "$daemon_pid" 2>/dev/null && wait "$daemon_pid" 2>/dev/null
  [ -n "$kv_home" ] && rm -rf "$kv_home"
  hdiutil detach -quiet "$mnt" >/dev/null 2>&1
  rmdir "$mnt" 2>/dev/null
}
trap cleanup EXIT

section "disk image: $dmg"
check "image signature (codesign --verify)" codesign --verify --strict --verbose=2 "$dmg"
check "image Gatekeeper, install assessment (spctl -t install)" spctl -a -vv -t install "$dmg"
check "image Gatekeeper, primary signature (spctl -t open)" \
  spctl -a -vv -t open --context context:primary-signature "$dmg"
check "image notarization ticket stapled" xcrun stapler validate "$dmg"
check "image team is $team" sh -c "codesign -dvv '$dmg' 2>&1 | grep -qx 'TeamIdentifier=$team'"

hdiutil attach -quiet -readonly -nobrowse -noautoopen -mountpoint "$mnt" "$dmg" ||
  { echo "FAIL could not mount $dmg"; exit 1; }
app="$(find "$mnt" -maxdepth 1 -name '*.app' | head -n 1)"
[ -n "$app" ] || { echo "FAIL no .app in $dmg"; exit 1; }

section "app: ${app##*/}"
printf '   version %s (%s), %s\n' \
  "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")" \
  "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$app/Contents/Info.plist")" \
  "$(/usr/libexec/PlistBuddy -c 'Print :CuaVersion' "$app/Contents/Info.plist" 2>/dev/null || echo 'no CuaVersion')"
check "app signature, deep and strict" codesign --verify --deep --strict --verbose=2 "$app"
check "app Gatekeeper (spctl)" spctl -a -vv "$app"
check "app notarization ticket stapled" xcrun stapler validate "$app"
# The Keyvault's production caller requirement (cua-keyvault caller.rs).
check "app meets the Keyvault requirement (team $team, com.trycua.spaces.macos)" \
  codesign --verify -R="anchor apple generic and certificate leaf[subject.OU] = \"$team\" and identifier \"com.trycua.spaces.macos\"" "$app"
check "bundled cua meets the Keyvault requirement (team $team, com.trycua.cua)" \
  codesign --verify -R="anchor apple generic and certificate leaf[subject.OU] = \"$team\" and identifier \"com.trycua.cua\"" "$app/Contents/MacOS/cua"

section "updater (Sparkle)"
plist="$app/Contents/Info.plist"
feed="$(/usr/libexec/PlistBuddy -c 'Print :SUFeedURL' "$plist" 2>/dev/null)"
printf '   SUFeedURL %s\n' "${feed:-(none)}"
check "SUFeedURL is https" sh -c "case '$feed' in https://*) exit 0 ;; *) exit 1 ;; esac"
check "SUPublicEDKey is a 32-byte Ed25519 key" \
  sh -c "/usr/libexec/PlistBuddy -c 'Print :SUPublicEDKey' '$plist' | base64 -d 2>/dev/null | wc -c | grep -qx ' *32'"
sparkle="$app/Contents/Frameworks/Sparkle.framework"
sparkle_parts=(
  "$sparkle/Versions/B/XPCServices/Installer.xpc"
  "$sparkle/Versions/B/XPCServices/Downloader.xpc"
  "$sparkle/Versions/B/Autoupdate"
  "$sparkle/Versions/B/Updater.app"
  "$sparkle"
)
for f in "${sparkle_parts[@]}"; do check "present: ${f#"$app"/}" test -e "$f"; done

for f in "$app" "$app/Contents/MacOS/cua" "$app/Contents/Frameworks/libcua_sdk.dylib" "${sparkle_parts[@]}"; do
  section "signature: ${f#"$mnt"/}"
  info="$(codesign -dvv "$f" 2>&1)"
  printf '%s\n' "$info" | grep -E '^(Identifier|Format|CodeDirectory|Authority|Timestamp|TeamIdentifier|Runtime Version)' | sed 's/^/   /'
  check "Developer ID Application authority" sh -c "printf '%s\n' \"\$1\" | grep -q '^Authority=Developer ID Application: .*($team)$'" _ "$info"
  check "team $team" sh -c "printf '%s\n' \"\$1\" | grep -qx 'TeamIdentifier=$team'" _ "$info"
  check "hardened runtime" sh -c "printf '%s\n' \"\$1\" | grep -q '^CodeDirectory.*flags=.*runtime'" _ "$info"
  check "secure timestamp" sh -c "printf '%s\n' \"\$1\" | grep -q '^Timestamp='" _ "$info"
done

for f in "$app" "$app/Contents/MacOS/cua"; do
  section "entitlements: ${f#"$mnt"/}"
  ents="$(codesign -d --entitlements - --xml "$f" 2>/dev/null | plutil -convert xml1 -o - - 2>/dev/null)"
  printf '%s\n' "${ents:-   (none)}" | sed 's/^/   /'
  check "no get-task-allow (debuggable builds are not notarizable)" \
    sh -c "! printf '%s' \"\$1\" | grep -q 'get-task-allow'" _ "$ents"
done

section "launch"
# Libraries resolve inside the bundle and pass library validation; the app
# starts and exits before any window (throwaway HOME, telemetry off).
check "the app launches (scripts/check-launch.sh)" "$(dirname "$0")/check-launch.sh" "$app"

section "architectures"
for f in "$app/Contents/MacOS/CuaSpacesMac" "$app/Contents/MacOS/cua" "$app/Contents/Frameworks/libcua_sdk.dylib" \
  "$sparkle/Versions/B/Sparkle" "$sparkle/Versions/B/Autoupdate"; do
  for a in $archs; do check "${f##*/} has $a ($(lipo -archs "$f"))" lipo "$f" -verify_arch "$a"; done
done

if [ "$smoke" = 1 ]; then
  section "Keyvault smoke (bundled cua daemon, throwaway CUA_HOME)"
  cua="$app/Contents/MacOS/cua"
  mkdir -p "$HOME/.cache"
  # Inside the real home: the Keyvault distrusts a CUA_HOME outside it.
  kv_home="$(mktemp -d "$HOME/.cache/cua-verify-release.XXXXXX")"
  chmod 700 "$kv_home"
  CUA_HOME="$kv_home" CUA_TELEMETRY=0 DO_NOT_TRACK=1 \
    "$cua" daemon start --foreground --socket "$kv_home/cua.sock" --loopback off \
    >"$kv_home/daemon.out" 2>&1 &
  daemon_pid=$!
  for _ in $(seq 1 80); do [ -S "$kv_home/keyvault.sock" ] && break; sleep 0.25; done
  check "the bundled daemon serves the Keyvault socket" test -S "$kv_home/keyvault.sock"
  check "cua keyvault status: client and daemon verify each other as Cua" \
    env CUA_HOME="$kv_home" CUA_TELEMETRY=0 DO_NOT_TRACK=1 \
    "$cua" keyvault status --daemon "$kv_home/cua.sock"
fi

echo
if [ $failed -eq 0 ]; then echo "ALL CHECKS PASSED"; else echo "SOME CHECKS FAILED"; fi
exit $failed

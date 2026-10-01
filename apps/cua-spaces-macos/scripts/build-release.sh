#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Builds the release "Cua Spaces.app" for macOS: the Cua Spaces app export
# (libcua_spaces_ffi, staged as libcua_sdk.dylib) and the Cua Spaces build of
# the `cua` CLI for each architecture, joined with lipo, then the SwiftUI app
# (scripts/build-app.sh release), the release version in Info.plist, and a
# hardened-runtime signature from the inside out:
#
#   Contents/Frameworks/Sparkle.framework  the updater, in Sparkle's documented
#                                          order: XPCServices/Installer.xpc,
#                                          XPCServices/Downloader.xpc (its
#                                          entitlements kept), Autoupdate,
#                                          Updater.app, then the framework
#   Contents/Frameworks/libcua_sdk.dylib   (no entitlements)
#   Contents/MacOS/cua                     com.trycua.cua, Support/cua.entitlements
#   Cua Spaces.app                         com.trycua.spaces.macos, Support/CuaSpacesMac.entitlements
#
#   scripts/build-release.sh --version 0.2.0[-suffix] [--build-number N]
#       [--arch universal|arm64|x86_64] [--sign IDENTITY] [--out DIR]
#       [--feed-url URL] [--skip-cargo]
#
# --feed-url sets the updater's appcast (SUFeedURL; default Support/Info.plist's,
# trycua/cua's rolling cua-spaces-latest release). The release workflow passes
# its own repository's, so a staging build updates from staging.
#
# --sign takes a codesign identity ("Developer ID Application: Name (TEAM)");
# the default "-" signs ad hoc (the hardened runtime and entitlements are
# still applied, so the bundle's layout and flags match a release). An ad
# hoc signature has no team, and the hardened runtime's library validation
# loads only the process's own team's libraries, so an ad hoc app also gets
# com.apple.security.cs.disable-library-validation, or it could not load
# its own libcua_sdk.dylib and Sparkle.framework; a Developer ID build never
# does. Every signature but an ad hoc one carries a secure timestamp. The
# result must pass scripts/check-launch.sh (every library resolves inside
# the bundle and passes library validation; the app starts and exits). Notarization and
# the DMG are separate steps (scripts/package-dmg.sh, cd-cua-spaces.yml).
#
# Cargo builds into CARGO_TARGET_DIR (default libs/cua/target) with
# CARGO_BUILD_JOBS as set. --skip-cargo reuses <target>/<triple>/release.
# Prints the app's path.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
repo="$(cd "$here/../.." && pwd)"
cua="$repo/libs/cua"

version=""
build_number=""
arch=universal
identity="-"
out="$here/.build/release-app"
skip_cargo=0
feed_url=""
while [ $# -gt 0 ]; do
  case "$1" in
    --version) version="$2"; shift ;;
    --build-number) build_number="$2"; shift ;;
    --arch) arch="$2"; shift ;;
    --sign) identity="$2"; shift ;;
    --out) out="$2"; shift ;;
    --skip-cargo) skip_cargo=1 ;;
    --feed-url) feed_url="$2"; shift ;;
    -h | --help) sed -n '5,37p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done

# CFBundleShortVersionString is X.Y.Z (numbers only); a prerelease suffix
# (0.2.0-staging.4) goes in CuaVersion. CFBundleVersion is X.Y.Z plus the
# build number when there is one (the release workflow's run number).
if [[ ! "$version" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)(-[0-9A-Za-z.-]+)?$ ]]; then
  echo "--version must look like 1.2.3 or 1.2.3-suffix" >&2
  exit 2
fi
short="${BASH_REMATCH[1]}.${BASH_REMATCH[2]}.${BASH_REMATCH[3]}"
bundle_version="$short"
if [ -n "$build_number" ]; then
  [[ "$build_number" =~ ^[0-9]+$ ]] || { echo "--build-number must be a number" >&2; exit 2; }
  bundle_version="$short.$build_number"
fi

case "$arch" in
  universal) archs="arm64 x86_64" ;;
  arm64 | x86_64) archs="$arch" ;;
  *) echo "--arch must be universal, arm64 or x86_64" >&2; exit 2 ;;
esac
triple() { [ "$1" = arm64 ] && echo aarch64-apple-darwin || echo x86_64-apple-darwin; }

target="${CARGO_TARGET_DIR:-$cua/target}"
case "$target" in /*) ;; *) target="$cua/$target" ;; esac
if [ "$skip_cargo" = 0 ]; then
  for a in $archs; do
    (cd "$cua" && cargo build --locked --release -p cua-spaces-ffi -p cua-spaces-cli --target "$(triple "$a")")
  done
fi

# One library and one CLI with every architecture, where stage-library.sh
# and build-app.sh look for them.
joined="$out/.joined"
rm -rf "$joined"
mkdir -p "$joined/release"
for f in libcua_spaces_ffi.dylib cua-spaces-cli; do
  slices=()
  for a in $archs; do
    s="$target/$(triple "$a")/release/$f"
    [ -f "$s" ] || { echo "missing $s (build it, or drop --skip-cargo)" >&2; exit 1; }
    slices+=("$s")
  done
  lipo -create "${slices[@]}" -output "$joined/release/$f"
done
CARGO_TARGET_DIR="$joined" "$repo/libs/spaces-app-swift/scripts/stage-library.sh"

CUA_APP_ARCHS="$archs" CUA_CLI_BIN="$joined/release/cua-spaces-cli" \
  "$here/scripts/build-app.sh" release "$out" >/dev/null
app="$out/Cua Spaces.app"
[ -x "$app/Contents/MacOS/cua" ] || { echo "$app has no bundled cua CLI" >&2; exit 1; }

plist="$app/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $short" "$plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $bundle_version" "$plist"
/usr/libexec/PlistBuddy -c "Delete :CuaVersion" "$plist" 2>/dev/null || true
/usr/libexec/PlistBuddy -c "Add :CuaVersion string $version" "$plist"
if [ -n "$feed_url" ]; then
  case "$feed_url" in https://*) ;; *) echo "--feed-url must be https" >&2; exit 2 ;; esac
  /usr/libexec/PlistBuddy -c "Set :SUFeedURL $feed_url" "$plist"
fi
for key in SUFeedURL SUPublicEDKey; do
  /usr/libexec/PlistBuddy -c "Print :$key" "$plist" >/dev/null 2>&1 || { echo "Info.plist has no $key" >&2; exit 1; }
done

# Inside out: nested code first, the bundle last (its signature seals the
# nested signatures). No --deep: each piece gets its own identifier and
# entitlements.
sign=(codesign --force --options runtime --sign "$identity")
[ "$identity" = "-" ] || sign+=(--timestamp)
# Sparkle, inside out, as its documentation orders it for a Developer ID
# app outside the App Sandbox: the XPC services (the Downloader keeps its
# entitlements), Autoupdate, Updater.app, then the framework. Each keeps
# Sparkle's own identifier.
sparkle="$app/Contents/Frameworks/Sparkle.framework"
sparkle_parts=(
  "$sparkle/Versions/B/XPCServices/Installer.xpc"
  "$sparkle/Versions/B/XPCServices/Downloader.xpc"
  "$sparkle/Versions/B/Autoupdate"
  "$sparkle/Versions/B/Updater.app"
  "$sparkle"
)
for f in "${sparkle_parts[@]}"; do
  [ -e "$f" ] || { echo "missing $f" >&2; exit 1; }
  case "$f" in
    *Downloader.xpc) "${sign[@]}" --preserve-metadata=entitlements "$f" ;;
    *) "${sign[@]}" "$f" ;;
  esac
done
"${sign[@]}" "$app/Contents/Frameworks/libcua_sdk.dylib"
"${sign[@]}" --identifier com.trycua.cua \
  --entitlements "$here/Support/cua.entitlements" "$app/Contents/MacOS/cua"
app_entitlements="$here/Support/CuaSpacesMac.entitlements"
if [ "$identity" = "-" ]; then
  app_entitlements="$out/.adhoc.entitlements"
  cp "$here/Support/CuaSpacesMac.entitlements" "$app_entitlements"
  /usr/libexec/PlistBuddy -c "Add :com.apple.security.cs.disable-library-validation bool true" "$app_entitlements"
fi
"${sign[@]}" --identifier com.trycua.spaces.macos --entitlements "$app_entitlements" "$app"
[ "$identity" = "-" ] && rm -f "$app_entitlements"

codesign --verify --deep --strict --verbose=2 "$app" >&2
for f in "$app" "$app/Contents/MacOS/cua" "$app/Contents/Frameworks/libcua_sdk.dylib" "${sparkle_parts[@]}"; do
  flags="$(codesign -dv "$f" 2>&1 | sed -n 's/^CodeDirectory.*flags=\([^ ]*\).*/\1/p')"
  case "$flags" in *runtime*) ;; *) echo "$f is not signed with the hardened runtime ($flags)" >&2; exit 1 ;; esac
done
[ "$(codesign -dv "$app/Contents/MacOS/cua" 2>&1 | sed -n 's/^Identifier=//p')" = com.trycua.cua ] ||
  { echo "the bundled cua is not signed as com.trycua.cua" >&2; exit 1; }
[ "$(codesign -dv "$app" 2>&1 | sed -n 's/^Identifier=//p')" = com.trycua.spaces.macos ] ||
  { echo "the app is not signed as com.trycua.spaces.macos" >&2; exit 1; }
# A Developer ID build must be first party to the Keyvault: the broker and
# its clients accept only code meeting TrustPolicy::production() (Apple
# anchored, the pinned team CUA_TEAM_ID in cua-keyvault's caller.rs, a Cua
# identifier, hardened runtime; checked above). Signing with another team
# would ship an app whose Keyvault answers "not signed by Cua".
if [ "$identity" != "-" ]; then
  team="$(sed -n 's/^pub const CUA_TEAM_ID: &str = "\([A-Z0-9]*\)";/\1/p' \
    "$cua/crates/cua-keyvault/src/caller.rs")"
  [ -n "$team" ] || { echo "could not read CUA_TEAM_ID from cua-keyvault" >&2; exit 1; }
  for pair in "$app:com.trycua.spaces.macos" "$app/Contents/MacOS/cua:com.trycua.cua"; do
    f="${pair%:*}"
    id="${pair##*:}"
    codesign --verify -R="anchor apple generic and certificate leaf[subject.OU] = \"$team\" and identifier \"$id\"" "$f" ||
      { echo "$f does not meet the Keyvault requirement (team $team, $id): sign with the Cua team's Developer ID" >&2; exit 1; }
  done
fi
# A Developer ID build signs Sparkle with the same team (library
# validation stays on: the app loads only its own team's code).
if [ "$identity" != "-" ]; then
  for f in "${sparkle_parts[@]}"; do
    codesign --verify -R="anchor apple generic and certificate leaf[subject.OU] = \"$team\"" "$f" ||
      { echo "$f is not signed by team $team" >&2; exit 1; }
  done
fi
# The binaries must not load anything outside the bundle and the OS.
for f in "$app/Contents/MacOS/CuaSpacesMac" "$app/Contents/MacOS/cua" "$app/Contents/Frameworks/libcua_sdk.dylib"; do
  bad="$(otool -L "$f" | awk '/^\t/ {print $1}' | sort -u |
    grep -vE '^(/usr/lib/|/System/Library/|@rpath/libcua_sdk\.dylib$|@rpath/Sparkle\.framework/Versions/B/Sparkle$)' || true)"
  [ -z "$bad" ] || { echo "$f links outside the bundle and the OS: $bad" >&2; exit 1; }
done
# It launches: every library resolves inside the bundle and passes library
# validation, and the app starts and exits (--check-launch-only).
"$here/scripts/check-launch.sh" "$app" >&2
rm -rf "$joined"
echo "$app"

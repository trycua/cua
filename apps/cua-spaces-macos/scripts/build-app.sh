#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Builds "Cua Spaces.app" (SwiftUI) from this package: the executable, the
# Cua Spaces app export dylib (libcua_spaces_ffi, which carries the cua SDK;
# staged as libs/cua/swift/lib/libcua_sdk.dylib by
# libs/spaces-app-swift/scripts/stage-library.sh), the app icon
# (Support/AppIcon.icns, the terracotta koala keycap; scripts/dmg-art builds it)
# and the menu bar template the Tauri app ships (apps/cua-spaces/src-tauri/
# icons), plus the `cua` CLI next to the executable when one is built
# (the app installs it onto PATH on first launch, like the Tauri app's
# sidecar), Sparkle.framework (the updater, from the pinned SwiftPM
# artifact, with its XPC services) in Contents/Frameworks and the
# third-party notices (THIRD_PARTY_NOTICES.md in Resources), the host
# service (the pinned cua-spacesd from scripts/fetch-spacesd.sh, or
# $CUA_SPACESD_BIN, in Contents/Helpers; the app's own executable runs it as
# its login item, whose SMAppService plist is in
# Contents/Library/LaunchAgents), then signs it ad hoc as
# com.trycua.spaces.macos.
#
# A release build strips every binary it ships (local symbols from the SDK
# dylib, all but the dynamic ones from the executables) after writing their
# dSYMs to <out-dir>/dSYMs, which crash reports symbolicate against.
#
#   scripts/build-app.sh [--channel stable|local] [debug|release] [out-dir]
#
# --channel local builds "Cua Spaces Local.app", which runs beside an
# installed Cua Spaces and cannot touch it (CUA-1776): the Local bundle id
# (com.trycua.spaces.macos.local), name, helper login item (label and plist file
# name), URL scheme (cua-local, not cua) and cua home (~/.cua-local), staging
# by default (CuaEnv), and no Sparkle feed or key, so it never offers or
# installs an update. Every name comes from CuaChannel (scripts/channel-names.sh,
# libs/cua/crates/cua-home/src/channel.rs); the Info.plist keys CuaChannel,
# CuaHome, CuaHelperLabel, CuaUrlScheme and CuaEnv tell the app and its
# helpers which channel they are. scripts/check-local-bundle.sh proves it, and
# runs at the end of this script. Without --channel (the default, stable) the
# bundle is exactly the one the release scripts sign and ship.
#
# CUA_APP_ARCHS="arm64 x86_64" builds each architecture and joins them with
# lipo (a universal app; the SDK dylib and the `cua` CLI must then be
# universal too). Unset, it builds for this Mac only.
#
# Release distribution (Developer ID, hardened runtime, notarization) is
# scripts/build-release.sh and the release workflow; this script is for
# development and CI builds.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
channel=stable
args=()
while [ $# -gt 0 ]; do
  case "$1" in
    --channel) [ $# -ge 2 ] || { echo "--channel needs stable or local" >&2; exit 2; }; channel="$2"; shift 2 ;;
    --channel=*) channel="${1#--channel=}"; shift ;;
    *) args+=("$1"); shift ;;
  esac
done
case "$channel" in
  stable | local) ;;
  *) echo "--channel must be stable or local (got '$channel')" >&2; exit 2 ;;
esac
set -- ${args[@]+"${args[@]}"}
config="${1:-debug}"
out="${2:-$here/.build/app}"
# The names this bundle carries. Stable's are the ones the release scripts
# and the updater know; Local's come from CuaChannel, never from here.
app_name="Cua Spaces"
bundle_id=com.trycua.spaces.macos
helper_label=com.trycua.spaces.helper
if [ "$channel" = local ]; then
  names="$("$here/scripts/channel-names.sh" local)"
  while IFS='=' read -r key value; do
    case "$key" in
      app_name) app_name="$value" ;;
      bundle_id) bundle_id="$value" ;;
      helper_label) helper_label="$value" ;;
      url_scheme) url_scheme="$value" ;;
      home) cua_home="$value" ;;
      env) cua_env="$value" ;;
    esac
  done <<<"$names"
  for v in app_name bundle_id helper_label url_scheme cua_home cua_env; do
    [ -n "${!v:-}" ] || { echo "scripts/channel-names.sh local printed no $v" >&2; exit 1; }
  done
fi
icons="$here/../cua-spaces/src-tauri/icons"
# CUA_VZ_BIN, CUA_MICROVM_INIT_BIN: the microVM monitor (cua-vz) and the
# static aarch64 guest init to bundle (build-release.sh builds both).
# CUA_NETGW_BIN: the microVMs' network gateway (cua-netgw) to bundle
# (libs/cua/netgw/build.sh).
# CUA_SPACESD_STATIC_DIR: a directory of static Linux cua-spacesd builds
# (cua-spacesd-linux-aarch64, cua-spacesd-linux-x86_64) to bundle: the
# daemon injects one into OCI images that carry none (the release workflow
# fetches them from the pinned cua-spacesd release).
# CUA_CLI_BIN: the `cua` to bundle (default: the Cua Spaces build of `cua`,
# `cargo build --release -p cua-spaces-cli`, when present).
cli="${CUA_CLI_BIN:-${CARGO_TARGET_DIR:-$here/../../libs/cua/target}/release/cua-spaces-cli}"
# CUA_SDK_DYLIB: another build of the library (a debug build for the
# development Keyvault path, which a release library refuses).
dylib="${CUA_SDK_DYLIB:-$here/../../libs/cua/swift/lib/libcua_sdk.dylib}"
[ -f "$dylib" ] || { echo "missing $dylib: run libs/spaces-app-swift/scripts/stage-library.sh" >&2; exit 1; }

# A regenerated binding needs every module recompiled (SwiftPM would reuse
# modules built against the old one; see fresh-abi.sh).
app_binding="$here/../../libs/spaces-app-swift/Sources"
export CUA_SWIFT_ABI_EXTRA="$app_binding/CuaSpacesFFI/CuaSpacesFFI.swift $app_binding/cua_spaces_ffiFFI/include/cua_spaces_ffiFFI.h"
archs="${CUA_APP_ARCHS:-}"
bins=()
if [ -z "$archs" ]; then
  "$here/../../libs/cua/swift/scripts/fresh-abi.sh" "$here" "$config"
  swift build --package-path "$here" -c "$config" --product CuaSpacesMac
  bins+=("$(swift build --package-path "$here" -c "$config" --show-bin-path)")
else
  for arch in $archs; do
    "$here/../../libs/cua/swift/scripts/fresh-abi.sh" "$here" "$config" --arch "$arch"
    swift build --package-path "$here" -c "$config" --arch "$arch" --product CuaSpacesMac
    bins+=("$(swift build --package-path "$here" -c "$config" --arch "$arch" --show-bin-path)")
  done
fi
# Resource bundles are the same for every architecture.
bin="${bins[0]}"

app="$out/$app_name.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$app/Contents/Frameworks"
cp "$here/Support/Info.plist" "$app/Contents/Info.plist"
# Crash reporting (CrashReporting.swift): the public DSN from the shared
# config. An empty or missing dsn writes an empty key, which turns it off.
crash_dsn="$(plutil -extract dsn raw -o - "$here/../../config/spaces-crash-reporting.json" 2>/dev/null || true)"
/usr/libexec/PlistBuddy -c "Delete :CuaCrashDSN" "$app/Contents/Info.plist" 2>/dev/null || true
/usr/libexec/PlistBuddy -c "Add :CuaCrashDSN string $crash_dsn" "$app/Contents/Info.plist"
if [ "$channel" = local ]; then
  # Another identity, no updater, staging, its own home, helper and URL
  # scheme. Nothing here may reach an installed Cua Spaces: see
  # scripts/check-local-bundle.sh.
  plist="$app/Contents/Info.plist"
  pb=/usr/libexec/PlistBuddy
  $pb -c "Set :CFBundleIdentifier $bundle_id" "$plist"
  $pb -c "Set :CFBundleName $app_name" "$plist"
  $pb -c "Set :CFBundleDisplayName $app_name" "$plist"
  for key in SUFeedURL SUPublicEDKey SUEnableAutomaticChecks SUAutomaticallyUpdate SUVerifyUpdateBeforeExtraction; do
    $pb -c "Delete :$key" "$plist"
  done
  # The `cua` scheme is Stable's (cua://add/<code>): Local answers cua-local://.
  $pb -c "Set :CFBundleURLTypes:0:CFBundleURLName $bundle_id" "$plist"
  $pb -c "Set :CFBundleURLTypes:0:CFBundleURLSchemes:0 $url_scheme" "$plist"
  for kv in "CuaChannel:local" "CuaEnv:$cua_env" "CuaHome:$cua_home" "CuaHelperLabel:$helper_label" "CuaUrlScheme:$url_scheme"; do
    $pb -c "Delete :${kv%%:*}" "$plist" 2>/dev/null || true
    $pb -c "Add :${kv%%:*} string ${kv#*:}" "$plist"
  done
  plutil -lint "$plist" >/dev/null
fi
if [ ${#bins[@]} -gt 1 ]; then
  slices=()
  for b in "${bins[@]}"; do slices+=("$b/CuaSpacesMac"); done
  lipo -create "${slices[@]}" -output "$app/Contents/MacOS/CuaSpacesMac"
else
  cp "$bin/CuaSpacesMac" "$app/Contents/MacOS/CuaSpacesMac"
fi
# SwiftPM resource bundles go in Contents/Resources (codesign refuses
# unsealed content at the bundle root). SwiftPM's generated Bundle.module
# never looks there (only the bundle root, then this machine's build
# directory), so the app finds them through ModuleResources instead.
for b in "$bin"/*.bundle; do [ -d "$b" ] && cp -R "$b" "$app/Contents/Resources/"; done
cp "$here/Support/AppIcon.icns" "$app/Contents/Resources/AppIcon.icns"
# macOS 26 draws icons without their own background on a grey system plate. The
# Icon Composer source (full-bleed background + key layer, dark/tinted/clear)
# compiles to Assets.car, found through CFBundleIconName; AppIcon.icns above is
# the fallback for macOS 15 and older.
icon_tmp="$(mktemp -d)"
if xcrun actool "$here/Support/AppIcon.icon" --compile "$icon_tmp" --platform macosx \
  --minimum-deployment-target 14.0 --app-icon AppIcon --include-all-app-icons \
  --output-partial-info-plist "$icon_tmp/partial.plist" >/dev/null 2>&1 && [ -s "$icon_tmp/Assets.car" ]; then
  cp "$icon_tmp/Assets.car" "$app/Contents/Resources/Assets.car"
else
  echo "warning: actool (Xcode 26+) could not compile Support/AppIcon.icon; macOS 26 will show the icon on a grey plate" >&2
  [ -z "${CI:-}" ] || exit 1
fi
rm -rf "$icon_tmp"
cp "$icons/tray-template.png" "$app/Contents/Resources/tray-template.png"
cp "$icons/tray-template@2x.png" "$app/Contents/Resources/tray-template@2x.png"
cp "$here/THIRD_PARTY_NOTICES.md" "$app/Contents/Resources/THIRD_PARTY_NOTICES.md"
# New UI (preview): the shared web UI's build (apps/cua-spaces-web, `dist`),
# served from Contents/Resources/WebUI at cua-spaces://app/. CUA_WEBUI_DIST
# points at another build; without one the window says how to add it.
webui="${CUA_WEBUI_DIST:-$here/../cua-spaces-web/dist}"
if [ -f "$webui/index.html" ]; then
  cp -R "$webui" "$app/Contents/Resources/WebUI"
else
  echo "note: no web UI build at $webui; New UI (preview) shows a placeholder" >&2
fi

# Sparkle, the updater: the framework SwiftPM linked (@rpath), with its
# symlinks (ditto), its XPC services, Autoupdate and Updater.app.
sparkle="$(find "$here/.build/artifacts/sparkle" -maxdepth 5 -type d -path '*macos-arm64_x86_64/Sparkle.framework' | head -n 1)"
[ -d "$sparkle" ] || { echo "missing Sparkle.framework under .build/artifacts/sparkle (swift package resolve)" >&2; exit 1; }
ditto "$sparkle" "$app/Contents/Frameworks/Sparkle.framework"

# The SDK dylib: load it from the bundle, not the source tree.
cp "$dylib" "$app/Contents/Frameworks/libcua_sdk.dylib"
install_name_tool -id "@rpath/libcua_sdk.dylib" "$app/Contents/Frameworks/libcua_sdk.dylib"
old="$(otool -L "$app/Contents/MacOS/CuaSpacesMac" | awk '/libcua_sdk/ {print $1; exit}')"
[ -n "$old" ] && install_name_tool -change "$old" "@rpath/libcua_sdk.dylib" "$app/Contents/MacOS/CuaSpacesMac"
# Only the bundle's copy: drop the development rpath into the source tree.
# (Each rpath once: a universal binary lists it per architecture, and one
# -delete_rpath removes it from every slice.)
for rp in $(otool -l "$app/Contents/MacOS/CuaSpacesMac" | awk '/LC_RPATH/ {getline; getline; print $2}' | sort -u); do
  case "$rp" in
    @*) ;;
    *) install_name_tool -delete_rpath "$rp" "$app/Contents/MacOS/CuaSpacesMac" ;;
  esac
done
install_name_tool -add_rpath "@executable_path/../Frameworks" "$app/Contents/MacOS/CuaSpacesMac" 2>/dev/null || true

# The host service: cua-spacesd in Contents/Helpers. The app's own
# executable runs it as the app's login item (SMAppService.agent;
# `CuaSpacesMac --service-helper run`), so macOS asks for and keeps its
# permissions under the app's own name, "Cua Spaces". Not in Contents/MacOS:
# AppKit takes the main bundle from the executable's path, so when the
# daemon's cursor overlay starts AppKit there it checks in with
# LaunchServices as the app itself, and opening Cua Spaces then activates
# the windowless daemon instead of launching the app (see the
# Contents/MacOS check below).
mkdir -p "$app/Contents/Library/LaunchAgents" "$app/Contents/Helpers"
# shellcheck disable=SC2086 # one word per architecture
spacesd="$("$here/scripts/fetch-spacesd.sh" ${archs:-$(uname -m)})"
cp "$spacesd" "$app/Contents/Helpers/cua-spacesd"
chmod 0755 "$app/Contents/Helpers/cua-spacesd"
helper_plist="$app/Contents/Library/LaunchAgents/$helper_label.plist"
cp "$here/Support/Helper/com.trycua.spaces.helper.plist" "$helper_plist"
if [ "$channel" = local ]; then
  # The login item's own label and the app it belongs to: its own launchd
  # job and its own entry in System Settings, never Stable's.
  /usr/libexec/PlistBuddy -c "Set :Label $helper_label" "$helper_plist"
  /usr/libexec/PlistBuddy -c "Set :AssociatedBundleIdentifiers:0 $bundle_id" "$helper_plist"
  plutil -lint "$helper_plist" >/dev/null
fi
[ -x "$cli" ] && cp "$cli" "$app/Contents/MacOS/cua"

# Release: symbols go to dSYMs, not into the download (about 40% of each
# binary). Executables keep only what dynamic linking needs; the dylib keeps
# its exported (global) symbols, which the app links against.
if [ "$config" = release ]; then
  syms="$out/dSYMs"
  rm -rf "$syms"
  mkdir -p "$syms"
  for f in "$app/Contents/MacOS/CuaSpacesMac" "$app/Contents/MacOS/cua" "$app/Contents/Helpers/cua-spacesd" \
    "$app/Contents/Frameworks/libcua_sdk.dylib"; do
    [ -f "$f" ] || continue
    # A prebuilt binary (cua-spacesd) has no debug map: its dSYM still
    # carries the symbol table, which is what symbolication needs.
    dsymutil "$f" -o "$syms/$(basename "$f").dSYM" 2>/dev/null
    case "$f" in
      *.dylib) strip -x "$f" 2>/dev/null ;;
      *) strip "$f" 2>/dev/null ;;
    esac
  done
fi
# The daemon keeps a signature of its own (ad hoc here; build-release.sh
# signs it with the release identity).
codesign -s - -f -i com.trycua.cua-env-driver "$app/Contents/Helpers/cua-spacesd"

if [ -x "$cli" ]; then
  codesign -s - -f -i com.trycua.cua "$app/Contents/MacOS/cua"
else
  echo "note: no cua CLI at $cli (cargo build --release -p cua-spaces-cli); the app will not install one" >&2
fi
# microVMs on Apple silicon: the monitor among the executables, the Linux
# guest init among the resources (it is not Mach-O code to sign).
if [ -n "${CUA_VZ_BIN:-}" ] && [ -n "${CUA_MICROVM_INIT_BIN:-}" ]; then
  cp "$CUA_VZ_BIN" "$app/Contents/MacOS/cua-vz"
  codesign -s - -f -i ai.cua.vz --entitlements "$here/../../libs/cua/vz/cua-vz.entitlements" \
    "$app/Contents/MacOS/cua-vz"
  cp "$CUA_MICROVM_INIT_BIN" "$app/Contents/Resources/cua-microvm-init"
fi
# The network gateway among the executables (found next to `cua`).
if [ -n "${CUA_NETGW_BIN:-}" ]; then
  cp "$CUA_NETGW_BIN" "$app/Contents/MacOS/cua-netgw"
  codesign -s - -f -i ai.cua.netgw "$app/Contents/MacOS/cua-netgw"
fi
# The static cua-spacesd builds among the resources (Linux executables, data
# to the bundle).
if [ -n "${CUA_SPACESD_STATIC_DIR:-}" ]; then
  found=0
  for f in "$CUA_SPACESD_STATIC_DIR"/cua-spacesd-linux-*; do
    [ -f "$f" ] || continue
    cp "$f" "$app/Contents/Resources/$(basename "$f")"
    found=1
  done
  [ "$found" = 1 ] || { echo "no cua-spacesd-linux-* in $CUA_SPACESD_STATIC_DIR" >&2; exit 1; }
fi
codesign -s - -f -i "$bundle_id" "$app/Contents/Frameworks/libcua_sdk.dylib"
# Sparkle keeps its upstream ad hoc signatures here (build-release.sh signs
# each piece with the release identity).
codesign -s - -f -i "$bundle_id" "$app"
codesign --verify --deep --strict "$app"

# The app must run on a Mac without this build directory: the resource
# bundle is in place and, in a release build, nothing in the executable
# points into $bin (a Bundle.module use would: its fallback is that absolute
# path). Debug builds keep the unused generated accessor, so only release
# builds (CI's) can check that. grep reads everything: with -q, strings
# would die of SIGPIPE and pipefail would hide the match.
for b in "$bin"/*.bundle; do
  [ -d "$b" ] || continue
  [ -d "$app/Contents/Resources/$(basename "$b")" ] || { echo "missing $(basename "$b") in Contents/Resources" >&2; exit 1; }
done
if [ "$config" = release ]; then
  for b in "${bins[@]}"; do
    if strings -a "$app/Contents/MacOS/CuaSpacesMac" | grep -F "$b/" >/dev/null; then
      echo "CuaSpacesMac references the build directory $b (Bundle.module?): it would crash on any other Mac; use ModuleResources" >&2
      exit 1
    fi
  done
fi
# Every requested architecture is in every binary (a thin dylib or CLI would
# only fail on the other kind of Mac).
if [ -n "$archs" ]; then
  for f in "$app/Contents/MacOS/CuaSpacesMac" "$app/Contents/Frameworks/libcua_sdk.dylib" "$app/Contents/MacOS/cua" \
    "$app/Contents/Frameworks/Sparkle.framework/Versions/B/Sparkle" "$app/Contents/Helpers/cua-spacesd"; do
    [ -e "$f" ] || continue
    for arch in $archs; do
      lipo "$f" -verify_arch "$arch" || { echo "$f has no $arch slice ($(lipo -archs "$f"))" >&2; exit 1; }
    done
  done
fi
# Only the app and the command-line tools are in Contents/MacOS
# (scripts/check-bundle-layout.sh: a background process there that starts
# AppKit registers as the app and takes over its launch).
"$here/scripts/check-bundle-layout.sh" "$app" >&2
# It launches (scripts/check-launch.sh: libraries resolve and load; the app
# starts and exits before any window).
"$here/scripts/check-launch.sh" "$app" >&2
if [ "$channel" = local ]; then
  "$here/scripts/check-local-bundle.sh" "$app" >&2
fi
echo "$app"

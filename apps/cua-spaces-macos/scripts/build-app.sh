#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Builds "Cua Spaces.app" (SwiftUI) from this package: the executable, the
# Cua Spaces app export dylib (libcua_spaces_ffi, which carries the cua SDK;
# staged as libs/cua/swift/lib/libcua_sdk.dylib by
# libs/spaces-app-swift/scripts/stage-library.sh), and the assets the
# Tauri app ships (apps/cua-spaces/src-tauri/icons: icon.icns and the menu
# bar template), plus the `cua` CLI next to the executable when one is built
# (the app installs it onto PATH on first launch, like the Tauri app's
# sidecar), Sparkle.framework (the updater, from the pinned SwiftPM
# artifact, with its XPC services) in Contents/Frameworks and the
# third-party notices (Settings, About, Acknowledgements), then signs it
# ad hoc as com.trycua.spaces.macos.
#
#   scripts/build-app.sh [debug|release] [out-dir]
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
config="${1:-debug}"
out="${2:-$here/.build/app}"
icons="$here/../cua-spaces/src-tauri/icons"
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

app="$out/Cua Spaces.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$app/Contents/Frameworks"
cp "$here/Support/Info.plist" "$app/Contents/Info.plist"
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
cp "$icons/icon.icns" "$app/Contents/Resources/AppIcon.icns"
cp "$icons/tray-template.png" "$app/Contents/Resources/tray-template.png"
cp "$icons/tray-template@2x.png" "$app/Contents/Resources/tray-template@2x.png"
cp "$here/THIRD_PARTY_NOTICES.md" "$app/Contents/Resources/THIRD_PARTY_NOTICES.md"

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

if [ -x "$cli" ]; then
  cp "$cli" "$app/Contents/MacOS/cua"
  codesign -s - -f -i com.trycua.cua "$app/Contents/MacOS/cua"
else
  echo "note: no cua CLI at $cli (cargo build --release -p cua-spaces-cli); the app will not install one" >&2
fi
codesign -s - -f -i com.trycua.spaces.macos "$app/Contents/Frameworks/libcua_sdk.dylib"
# Sparkle keeps its upstream ad hoc signatures here (build-release.sh signs
# each piece with the release identity).
codesign -s - -f -i com.trycua.spaces.macos "$app"
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
    "$app/Contents/Frameworks/Sparkle.framework/Versions/B/Sparkle"; do
    [ -e "$f" ] || continue
    for arch in $archs; do
      lipo "$f" -verify_arch "$arch" || { echo "$f has no $arch slice ($(lipo -archs "$f"))" >&2; exit 1; }
    done
  done
fi
# It launches (scripts/check-launch.sh: libraries resolve and load; the app
# starts and exits before any window).
"$here/scripts/check-launch.sh" "$app" >&2
echo "$app"

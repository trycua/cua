#!/usr/bin/env bash
# Build CuaSDKFFI.xcframework (static libcua_sdk.a + UniFFI header and
# modulemap) for the Swift package's `.binaryTarget`.
#
# Usage: scripts/build-xcframework.sh [--ios] [--out DIR] [--zip]
#   macOS slice: aarch64-apple-darwin plus x86_64-apple-darwin when that
#                rust target is installed (lipo'd into one universal .a).
#   --ios:       also aarch64-apple-ios and an iOS simulator slice.
#   --zip:       zip it and print the SwiftPM checksum (release upload).
# Uses `xcodebuild -create-xcframework` when Xcode is present and writes the
# (documented, stable) XCFramework layout by hand otherwise.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
swift_root="$(cd "$here/.." && pwd)"
cua_root="$(cd "$swift_root/.." && pwd)"
out="$swift_root/build"
ios=0
zip=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --ios) ios=1 ;;
    --zip) zip=1 ;;
    --out) out="$2"; shift ;;
    *) echo "unknown argument $1" >&2; exit 2 ;;
  esac
  shift
done

target_dir="${CARGO_TARGET_DIR:-$cua_root/target}"
installed="$(rustup target list --installed 2>/dev/null || true)"
build() {
  (cd "$cua_root" && cargo build --locked --release -p cua-sdk --target "$1")
}

mac_targets=(aarch64-apple-darwin)
grep -qx x86_64-apple-darwin <<<"$installed" && mac_targets+=(x86_64-apple-darwin)
for t in "${mac_targets[@]}"; do build "$t"; done

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
headers="$work/Headers"
mkdir -p "$headers"
cp "$swift_root/Sources/cua_sdkFFI/include/cua_sdkFFI.h" "$headers/"
cat > "$headers/module.modulemap" <<'MAP'
module cua_sdkFFI {
    header "cua_sdkFFI.h"
    export *
}
MAP

libs=()
for t in "${mac_targets[@]}"; do libs+=("$target_dir/$t/release/libcua_sdk.a"); done
mkdir -p "$work/macos"
lipo -create "${libs[@]}" -output "$work/macos/libcua_sdk.a"
mac_archs="$(lipo -archs "$work/macos/libcua_sdk.a" | tr ' ' '_')"

slices=("macos|$work/macos/libcua_sdk.a|macos|$mac_archs|")
if [[ $ios == 1 ]]; then
  build aarch64-apple-ios
  build aarch64-apple-ios-sim
  mkdir -p "$work/ios" "$work/ios-sim"
  cp "$target_dir/aarch64-apple-ios/release/libcua_sdk.a" "$work/ios/"
  cp "$target_dir/aarch64-apple-ios-sim/release/libcua_sdk.a" "$work/ios-sim/"
  slices+=("ios|$work/ios/libcua_sdk.a|ios|arm64|")
  slices+=("ios-sim|$work/ios-sim/libcua_sdk.a|ios|arm64|simulator")
fi

xcf="$out/CuaSDKFFI.xcframework"
rm -rf "$xcf"
mkdir -p "$out"
if command -v xcodebuild >/dev/null && xcodebuild -version >/dev/null 2>&1; then
  args=()
  for s in "${slices[@]}"; do
    IFS='|' read -r _ lib _ _ _ <<<"$s"
    args+=(-library "$lib" -headers "$headers")
  done
  xcodebuild -create-xcframework "${args[@]}" -output "$xcf"
else
  entries=""
  for s in "${slices[@]}"; do
    IFS='|' read -r _ lib platform archs variant <<<"$s"
    id="$platform-$archs${variant:+-$variant}"
    mkdir -p "$xcf/$id"
    cp "$lib" "$xcf/$id/libcua_sdk.a"
    cp -R "$headers" "$xcf/$id/Headers"
    arch_items=""
    for a in ${archs//_/ }; do arch_items+="<string>$a</string>"; done
    entries+="<dict><key>BinaryPath</key><string>libcua_sdk.a</string>"
    entries+="<key>HeadersPath</key><string>Headers</string>"
    entries+="<key>LibraryIdentifier</key><string>$id</string>"
    entries+="<key>LibraryPath</key><string>libcua_sdk.a</string>"
    entries+="<key>SupportedArchitectures</key><array>$arch_items</array>"
    entries+="<key>SupportedPlatform</key><string>$platform</string>"
    [[ -n "$variant" ]] && entries+="<key>SupportedPlatformVariant</key><string>$variant</string>"
    entries+="</dict>"
  done
  cat > "$xcf/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
<key>AvailableLibraries</key><array>$entries</array>
<key>CFBundlePackageType</key><string>XFWK</string>
<key>XCFrameworkFormatVersion</key><string>1.0</string>
</dict>
</plist>
PLIST
fi
echo "built $xcf"

if [[ $zip == 1 ]]; then
  (cd "$out" && rm -f CuaSDKFFI.xcframework.zip && zip -qry CuaSDKFFI.xcframework.zip CuaSDKFFI.xcframework)
  echo "checksum $(cd "$swift_root" && swift package compute-checksum "$out/CuaSDKFFI.xcframework.zip")"
fi

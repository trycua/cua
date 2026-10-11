#!/usr/bin/env bash
# Build the host cua-sdk static library and wrap it in the XCFramework the
# cua Swift package picks up with CUA_SWIFT_XCFRAMEWORK. Same layout as
# libs/cua/swift/scripts/build-xcframework.sh, but host-only and from the
# plain `target/release` directory, so it shares artifacts with other
# release builds of the workspace.
#
# Output: libs/cua/swift/build/CuaSDKFFI.xcframework (git-ignored there;
# the binary target path must live inside the Swift package).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../../../.." && pwd)"
cua_root="$repo/libs/cua"
swift_root="$cua_root/swift"
target_dir="${CARGO_TARGET_DIR:-$cua_root/target}"

(cd "$cua_root" && CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" cargo build --locked --release -p cua-sdk)

lib="$target_dir/release/libcua_sdk.a"
[ -f "$lib" ] || { echo "missing $lib" >&2; exit 1; }
arch="$(lipo -archs "$lib" | tr ' ' '_')"
xcf="$swift_root/build/CuaSDKFFI.xcframework"
id="macos-$arch"
rm -rf "$xcf"
mkdir -p "$xcf/$id/Headers"
cp "$lib" "$xcf/$id/libcua_sdk.a"
cp "$swift_root/Sources/cua_sdkFFI/include/cua_sdkFFI.h" "$xcf/$id/Headers/"
cat >"$xcf/$id/Headers/module.modulemap" <<'MAP'
module cua_sdkFFI {
    header "cua_sdkFFI.h"
    export *
}
MAP
items=""
for a in ${arch//_/ }; do items+="<string>$a</string>"; done
cat >"$xcf/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
<key>AvailableLibraries</key><array><dict>
<key>BinaryPath</key><string>libcua_sdk.a</string>
<key>HeadersPath</key><string>Headers</string>
<key>LibraryIdentifier</key><string>$id</string>
<key>LibraryPath</key><string>libcua_sdk.a</string>
<key>SupportedArchitectures</key><array>$items</array>
<key>SupportedPlatform</key><string>macos</string>
</dict></array>
<key>CFBundlePackageType</key><string>XFWK</string>
<key>XCFrameworkFormatVersion</key><string>1.0</string>
</dict>
</plist>
PLIST
echo "built $xcf"

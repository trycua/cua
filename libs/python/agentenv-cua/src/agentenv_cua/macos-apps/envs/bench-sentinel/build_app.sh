#!/bin/bash
# Build one bench app from the swift/ sources the install brought, and sign it ad hoc, as the bench's own build.sh does
# for both. Usage: build_app.sh <Name> <bundle id> <display name> <outdir>
set -euo pipefail
name=$1 bundle_id=$2 display=$3 out=$4
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
app="$out/$name.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
# Compile to a neutral path first: a process sweep by command-line pattern must not match the compiler.
tmpbin="$(mktemp -t benchbuild)"
xcrun swiftc -O -parse-as-library -swift-version 5 -target arm64-apple-macosx26.0 \
  "$here/swift/$name"*.swift -o "$tmpbin"
mv "$tmpbin" "$app/Contents/MacOS/$name"
chmod +x "$app/Contents/MacOS/$name"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>$bundle_id</string>
  <key>CFBundleName</key><string>$display</string>
  <key>CFBundleDisplayName</key><string>$display</string>
  <key>CFBundleExecutable</key><string>$name</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleShortVersionString</key><string>1.0</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>LSMinimumSystemVersion</key><string>26.0</string>
  <key>NSPrincipalClass</key><string>NSApplication</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticTermination</key><false/>
  <key>NSSupportsSuddenTermination</key><false/>
  <key>NSAppSleepDisabled</key><true/>
</dict>
</plist>
PLIST
# Ad hoc only: never a named identity, so the login keychain isn't involved.
codesign --force --sign - --timestamp=none "$app" >/dev/null 2>&1 || { echo "codesign failed for $app" >&2; exit 1; }
codesign --verify --strict "$app"
echo "$app"

#!/usr/bin/env bash
# Build FocusMonitorApp.app — counts focus-loss events for integration tests.
set -euo pipefail
cd "$(dirname "$0")"

BUNDLE=FocusMonitorApp.app
EXE=$BUNDLE/Contents/MacOS/FocusMonitorApp

rm -rf "$BUNDLE"
mkdir -p "$BUNDLE/Contents/MacOS"

xcrun swiftc \
    -target arm64-apple-macos14.0 \
    -framework AppKit \
    FocusMonitorApp.swift \
    -o "$EXE"

cat > "$BUNDLE/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key>   <string>com.trycua.FocusMonitorApp</string>
    <key>CFBundleName</key>         <string>FocusMonitorApp</string>
    <key>CFBundleExecutable</key>   <string>FocusMonitorApp</string>
    <key>NSPrincipalClass</key>     <string>NSApplication</string>
    <key>CFBundleVersion</key>      <string>1</string>
    <key>LSMinimumSystemVersion</key> <string>14.0</string>
</dict>
</plist>
PLIST

codesign --force --sign - "$BUNDLE"
echo "Built: $(pwd)/$BUNDLE"

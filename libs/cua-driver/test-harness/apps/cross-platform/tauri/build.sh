#!/usr/bin/env bash
# Stage the Tauri test harness for cua-driver tests on Linux and macOS.
#
# The app embeds the same shared/web/index.html DOM used by Electron,
# WebView2, and WKWebView. Build output is copied into
# rust/test-apps/harness-tauri/ with deterministic executable names.
set -euo pipefail

tauriDir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
crossDir="$(dirname "$tauriDir")"
appsDir="$(dirname "$crossDir")"
harnessDir="$(dirname "$appsDir")"
cuaDriverDir="$(dirname "$harnessDir")"
outDir="$cuaDriverDir/rust/test-apps/harness-tauri"
platform="$(uname -s)"

if ! command -v cargo >/dev/null 2>&1; then
  echo "[ERROR] cargo not on PATH. Install Rust first." >&2
  exit 1
fi

mkdir -p "$tauriDir/web"
cp -r "$harnessDir/shared/web/." "$tauriDir/web/"

echo "[BUILD] cargo build --release ..."
CARGO_TARGET_DIR="$tauriDir/src-tauri/target" \
  cargo build --release --manifest-path "$tauriDir/src-tauri/Cargo.toml"

rm -rf "$outDir"
mkdir -p "$outDir"

if [ "$platform" = "Darwin" ]; then
  srcBin="$tauriDir/src-tauri/target/release/cua-test-harness-tauri"
  [ -x "$srcBin" ] || { echo "[ERROR] Tauri binary missing after build" >&2; exit 1; }

  bundle="$outDir/CuaTestHarness.Tauri.app"
  appMacOS="$bundle/Contents/MacOS"
  mkdir -p "$appMacOS" "$bundle/Contents/Resources"
  cp "$srcBin" "$appMacOS/CuaTestHarness.Tauri"
  chmod +x "$appMacOS/CuaTestHarness.Tauri"

  cat > "$bundle/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key><string>CuaTestHarness.Tauri</string>
    <key>CFBundleIdentifier</key><string>com.trycua.harness.tauri</string>
    <key>CFBundleName</key><string>CuaTestHarness.Tauri</string>
    <key>CFBundleDisplayName</key><string>CuaTestHarness.Tauri</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>1.0.0</string>
    <key>CFBundleVersion</key><string>1</string>
    <key>LSMinimumSystemVersion</key><string>13.0</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
  xattr -cr "$bundle" 2>/dev/null || true
  echo "[OK]    Staged: $bundle"
else
  srcBin="$tauriDir/src-tauri/target/release/cua-test-harness-tauri"
  [ -x "$srcBin" ] || { echo "[ERROR] Tauri binary missing after build" >&2; exit 1; }

  cp "$srcBin" "$outDir/CuaTestHarness.Tauri"
  chmod +x "$outDir/CuaTestHarness.Tauri"
  echo "[OK]    Staged: $outDir/CuaTestHarness.Tauri"
fi

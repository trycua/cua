#!/usr/bin/env bash
# build.sh — stage the Electron test harness for cua-driver tests / modality
# recordings on Linux and macOS.
#
# On Linux we run the app straight from node_modules (electron's prebuilt
# linux-x64 binary) rather than packaging — the modality recorder launches it
# with `electron . --no-sandbox --disable-gpu --force-renderer-accessibility`
# under Xvfb + a dbus session so Chromium's web-AX tree registers on AT-SPI.
#
# On macOS we stage Electron.app as CuaTestHarness.Electron.app and place the
# same app resources under Contents/Resources/app.
set -euo pipefail

elecDir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
crossDir="$(dirname "$elecDir")"
appsDir="$(dirname "$crossDir")"
harnessDir="$(dirname "$appsDir")"
cuaDriverDir="$(dirname "$harnessDir")"
outDir="$cuaDriverDir/rust/test-apps/harness-electron"
platform="$(uname -s)"

if ! command -v npm >/dev/null 2>&1; then
  echo "[ERROR] npm not on PATH. Install Node.js first." >&2
  exit 1
fi

cd "$elecDir"

# stage the shared web harness (same index.html the WebView2 harness loads)
mkdir -p "$elecDir/web"
cp -r "$harnessDir/shared/web/." "$elecDir/web/"

# install the Electron runtime for the current platform
if [ "$platform" = "Darwin" ]; then
  electronBin="$elecDir/node_modules/electron/dist/Electron.app/Contents/MacOS/Electron"
else
  electronBin="$elecDir/node_modules/electron/dist/electron"
fi
if [ ! -x "$electronBin" ]; then
  echo "[INSTALL] npm install (first run)..."
  npm install --silent
fi

rm -rf "$outDir"
mkdir -p "$outDir"

if [ "$platform" = "Darwin" ]; then
  electronApp="$elecDir/node_modules/electron/dist/Electron.app"
  electronBin="$electronApp/Contents/MacOS/Electron"
  [ -x "$electronBin" ] || { echo "[ERROR] Electron.app missing after npm install" >&2; exit 1; }

  bundle="$outDir/CuaTestHarness.Electron.app"
  echo "[STAGE] Copying Electron.app + app resources to $bundle ..."
  cp -R "$electronApp" "$bundle"
  appDir="$bundle/Contents/Resources/app"
  rm -rf "$appDir"
  mkdir -p "$appDir"
  cp "$elecDir/main.js" "$appDir/"
  cp "$elecDir/package.json" "$appDir/"
  cp -R "$elecDir/web" "$appDir/web"

  cat > "$bundle/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key><string>Electron</string>
    <key>CFBundleIdentifier</key><string>com.trycua.harness.electron</string>
    <key>CFBundleName</key><string>CuaTestHarness.Electron</string>
    <key>CFBundleDisplayName</key><string>CuaTestHarness.Electron</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>1.0.0</string>
    <key>CFBundleVersion</key><string>1</string>
    <key>LSMinimumSystemVersion</key><string>13.0</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
  xattr -cr "$bundle" 2>/dev/null || true
  chmod +x "$bundle/Contents/MacOS/Electron"
  cat <<EOF
[OK]    Staged: $bundle
[NOTE]  Launch with:
          "$bundle/Contents/MacOS/Electron" --force-renderer-accessibility
EOF
else
  electronBin="$elecDir/node_modules/electron/dist/electron"
  [ -x "$electronBin" ] || { echo "[ERROR] electron binary missing after npm install" >&2; exit 1; }

  # flat stage with a deterministic exe name (parity with build.ps1)
  echo "[STAGE] Copying electron runtime + app to $outDir ..."
  cp -r "$elecDir/node_modules/electron/dist/." "$outDir/"
  appDir="$outDir/resources/app"
  mkdir -p "$appDir"
  cp "$elecDir/main.js" "$appDir/"
  cp "$elecDir/package.json" "$appDir/"
  cp -r "$elecDir/web" "$appDir/web"
  mv "$outDir/electron" "$outDir/CuaTestHarness.Electron"
  chmod +x "$outDir/CuaTestHarness.Electron"

  cat <<EOF
[OK]    Staged: $outDir/CuaTestHarness.Electron
[NOTE]  Launch under Xvfb + dbus with the Chromium flags Linux needs:
          CuaTestHarness.Electron --no-sandbox --disable-gpu --force-renderer-accessibility
        (--force-renderer-accessibility is what populates the web-AX tree on AT-SPI.)
        Or run in place from this dir:  npm run start:linux
EOF
fi

#!/usr/bin/env bash
# build.sh — stage the Electron test fixture app for cua-driver tests / modality
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
cuaDriverDir="$(cd "$harnessDir/../.." && pwd)"
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
electron_install_complete() {
  if [ "$platform" = "Darwin" ]; then
    [ -x "$electronBin" ] &&
      [ -d "$elecDir/node_modules/electron/dist/Electron.app/Contents/Frameworks" ] &&
      [ -f "$elecDir/node_modules/electron/dist/version" ] &&
      [ -f "$elecDir/node_modules/electron/path.txt" ]
  else
    [ -x "$electronBin" ] &&
      [ -f "$elecDir/node_modules/electron/dist/version" ] &&
      [ -f "$elecDir/node_modules/electron/path.txt" ]
  fi
}

if ! electron_install_complete; then
  echo "[INSTALL] npm install (first run)..."
  npm install --silent
fi

# npm 11 can leave package lifecycle scripts pending approval. In that case
# npm reports success but Electron's postinstall has not downloaded the real
# runtime. Run the pinned package installer explicitly after verifying the
# runtime shape so a tiny executable stub cannot be staged as a harness.
if ! electron_install_complete; then
  echo "[INSTALL] Electron runtime incomplete; running the pinned installer..."
  rm -rf "$elecDir/node_modules/electron/dist"
  node "$elecDir/node_modules/electron/install.js"
fi

# Node 26 + the current extract-zip dependency can still leave the macOS
# Frameworks payload out while returning success. Fall back to extracting the
# same official artifact downloaded by @electron/get; this is deterministic
# and works with the zip layout used by the pinned Electron version.
if ! electron_install_complete; then
  echo "[INSTALL] Electron helper produced an incomplete archive; extracting the official artifact..."
  electron_platform="linux"
  electron_path="electron"
  if [ "$platform" = "Darwin" ]; then
    electron_platform="darwin"
    electron_path="Electron.app/Contents/MacOS/Electron"
  fi
  electron_zip="$({
    ELECTRON_PLATFORM="$electron_platform" node <<'NODE'
const { downloadArtifact } = require('@electron/get');
const platform = process.env.ELECTRON_PLATFORM;
downloadArtifact({
  version: require('./node_modules/electron/package.json').version,
  artifactName: 'electron',
  platform,
  arch: process.arch,
}).then(path => process.stdout.write(path)).catch(error => {
  console.error(error.stack || error);
  process.exit(1);
});
NODE
  })"
  rm -rf "$elecDir/node_modules/electron/dist"
  mkdir -p "$elecDir/node_modules/electron/dist"
  unzip -q "$electron_zip" -d "$elecDir/node_modules/electron/dist"
  printf '%s' "$electron_path" > "$elecDir/node_modules/electron/path.txt"
  printf 'v%s' "$(node -p "require('./node_modules/electron/package.json').version")" \
    > "$elecDir/node_modules/electron/dist/version"
fi

if ! electron_install_complete; then
  echo "[ERROR] Electron runtime is incomplete after install" >&2
  exit 1
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
  cp "$elecDir/preload.js" "$appDir/"
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
  cp "$elecDir/preload.js" "$appDir/"
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

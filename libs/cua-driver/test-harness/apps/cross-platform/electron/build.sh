#!/usr/bin/env bash
# build.sh — Linux analogue of build.ps1: stage the Electron test harness for
# cua-driver tests / modality recordings.
#
# On Linux we run the app straight from node_modules (electron's prebuilt
# linux-x64 binary) rather than packaging — the modality recorder launches it
# with `electron . --no-sandbox --disable-gpu --force-renderer-accessibility`
# under Xvfb + a dbus session so Chromium's web-AX tree registers on AT-SPI.
# This script just makes sure web/ is staged and the electron runtime is
# installed, then (optionally) stages a flat folder with the binary renamed to
# CuaTestHarness.Electron for a deterministic process name — parity with
# build.ps1's output under rust/test-apps/harness-electron/.
set -euo pipefail

elecDir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
crossDir="$(dirname "$elecDir")"
appsDir="$(dirname "$crossDir")"
harnessDir="$(dirname "$appsDir")"
cuaDriverDir="$(dirname "$harnessDir")"
outDir="$cuaDriverDir/rust/test-apps/harness-electron"

if ! command -v npm >/dev/null 2>&1; then
  echo "[ERROR] npm not on PATH. Install Node.js first (e.g. sudo apt-get install -y nodejs npm)." >&2
  exit 1
fi

cd "$elecDir"

# stage the shared web harness (same index.html the WebView2 harness loads)
mkdir -p "$elecDir/web"
cp -r "$harnessDir/shared/web/." "$elecDir/web/"

# install the electron runtime (downloads the linux-x64 prebuilt binary)
if [ ! -x "$elecDir/node_modules/electron/dist/electron" ]; then
  echo "[INSTALL] npm install (first run)..."
  npm install --silent
fi
electronBin="$elecDir/node_modules/electron/dist/electron"
[ -x "$electronBin" ] || { echo "[ERROR] electron binary missing after npm install" >&2; exit 1; }

# optional flat stage with a deterministic exe name (parity with build.ps1)
echo "[STAGE] Copying electron runtime + app to $outDir ..."
rm -rf "$outDir"
mkdir -p "$outDir"
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

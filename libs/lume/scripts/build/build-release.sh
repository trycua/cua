#!/bin/sh

# Get the directory where this script is located
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# Navigate to the lume root directory (two levels up from scripts/build/)
LUME_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

cd "$LUME_DIR"

swift build -c release --product lume

# Assemble .app bundle
APP_BUNDLE=".release/lume.app"
mkdir -p "$APP_BUNDLE/Contents/MacOS"

cp -f .build/release/lume "$APP_BUNDLE/Contents/MacOS/lume"

# Copy resource bundle to Contents/Resources/ (not MacOS/ — flat bundles
# without Info.plist cause codesign to fail with "bundle format unrecognized")
mkdir -p "$APP_BUNDLE/Contents/Resources"
if [ -d ".build/release/lume_lume.bundle" ]; then
  cp -rf .build/release/lume_lume.bundle "$APP_BUNDLE/Contents/Resources/"
fi

# Stamp Info.plist with version from VERSION file
VERSION=$(cat VERSION 2>/dev/null || echo "0.0.0")
sed "s/__VERSION__/$VERSION/g" "./resources/Info.plist" > "$APP_BUNDLE/Contents/Info.plist"

# Embed provisioning profile if available
if [ -f "./resources/embedded.provisionprofile" ]; then
  cp "./resources/embedded.provisionprofile" "$APP_BUNDLE/Contents/embedded.provisionprofile"
fi

# Ad-hoc sign the bundle
codesign --force --entitlements ./resources/lume.entitlements --sign - "$APP_BUNDLE/Contents/MacOS/lume"
codesign --force --sign - "$APP_BUNDLE"

# Create wrapper script
mkdir -p .release
cat > .release/lume <<'WRAPPER_EOF'
#!/bin/sh
exec "$(dirname "$0")/lume.app/Contents/MacOS/lume" "$@"
WRAPPER_EOF
chmod +x .release/lume

# Install to user-local bin directory (standard location)
USER_BIN="$HOME/.local/bin"
APP_INSTALL_DIR="$HOME/.local/share/lume"

mkdir -p "$USER_BIN"
mkdir -p "$APP_INSTALL_DIR"

# Install .app bundle
rm -rf "$APP_INSTALL_DIR/lume.app"
cp -R ".release/lume.app" "$APP_INSTALL_DIR/"

# Create wrapper script in bin directory
cat > "$USER_BIN/lume" <<WRAPPER_EOF
#!/bin/sh
exec "$APP_INSTALL_DIR/lume.app/Contents/MacOS/lume" "\$@"
WRAPPER_EOF
chmod +x "$USER_BIN/lume"

# Advise user to add to PATH if not present
if ! echo "$PATH" | grep -q "$USER_BIN"; then
  echo "[lume build] Note: $USER_BIN is not in your PATH. Add 'export PATH=\"$USER_BIN:\$PATH\"' to your shell profile."
fi

#!/bin/bash

# Get the directory where this script is located
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# Navigate to the lume root directory (two levels up from scripts/build/)
LUME_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Set default log level if not provided
LOG_LEVEL=${LOG_LEVEL:-"normal"}

# Function to log based on level
log() {
  local level=$1
  local message=$2

  case "$LOG_LEVEL" in
    "minimal")
      # Only show essential or error messages
      if [ "$level" = "essential" ] || [ "$level" = "error" ]; then
        echo "$message"
      fi
      ;;
    "none")
      # Show nothing except errors
      if [ "$level" = "error" ]; then
        echo "$message" >&2
      fi
      ;;
    *)
      # Normal logging - show everything
      echo "$message"
      ;;
  esac
}

# Check required environment variables
required_vars=(
  "CERT_APPLICATION_NAME"
  "CERT_INSTALLER_NAME"
  "APPLE_ID"
  "TEAM_ID"
  "APP_SPECIFIC_PASSWORD"
)

for var in "${required_vars[@]}"; do
  if [ -z "${!var}" ]; then
    log "error" "Error: $var is not set"
    exit 1
  fi
done

# Get VERSION from environment or use default
VERSION=${VERSION:-"0.1.0"}

# Move to the project root directory
cd "$LUME_DIR"

# Ensure .release directory exists and is clean
mkdir -p .release
log "normal" "Ensuring .release directory exists and is accessible"

# Build the release version
log "essential" "Building release version..."
swift build -c release --product lume > /dev/null

# --- Assemble .app bundle ---
log "essential" "Assembling .app bundle..."

APP_BUNDLE=".release/lume.app"
rm -rf "$APP_BUNDLE"
mkdir -p "$APP_BUNDLE/Contents/MacOS"

# Copy the binary into the bundle
cp -f .build/release/lume "$APP_BUNDLE/Contents/MacOS/lume"

# Copy resource bundle (contains unattended presets) alongside the executable
# so SPM's Bundle.module resolves correctly
BUILD_BUNDLE=".build/release/lume_lume.bundle"
if [ -d "$BUILD_BUNDLE" ]; then
  cp -rf "$BUILD_BUNDLE" "$APP_BUNDLE/Contents/MacOS/"
fi

# Stamp and copy Info.plist
sed "s/__VERSION__/$VERSION/g" "./resources/Info.plist" > "$APP_BUNDLE/Contents/Info.plist"

# Embed the provisioning profile
PROVISION_PROFILE="./resources/embedded.provisionprofile"
if [ -f "$PROVISION_PROFILE" ]; then
  cp "$PROVISION_PROFILE" "$APP_BUNDLE/Contents/embedded.provisionprofile"
else
  log "error" "Error: embedded.provisionprofile not found at $PROVISION_PROFILE"
  log "error" "The provisioning profile is required for the com.apple.vm.networking entitlement."
  log "error" "Obtain one from the Apple Developer portal tied to bundle ID com.trycua.lume."
  exit 1
fi

# --- Sign the .app bundle ---
log "essential" "Signing .app bundle..."

# Sign the binary inside the bundle first (with entitlements)
codesign --force --options runtime --timestamp \
         --entitlements ./resources/lume.entitlements \
         --sign "$CERT_APPLICATION_NAME" \
         "$APP_BUNDLE/Contents/MacOS/lume"

# Sign the outer bundle
codesign --force --options runtime --timestamp \
         --sign "$CERT_APPLICATION_NAME" \
         "$APP_BUNDLE"

# --- Package as .pkg installer ---
log "essential" "Building installer package..."

TEMP_ROOT=$(mktemp -d)
mkdir -p "$TEMP_ROOT/usr/local/share/lume"
cp -R "$APP_BUNDLE" "$TEMP_ROOT/usr/local/share/lume/"

if ! pkgbuild --root "$TEMP_ROOT" \
         --identifier "com.trycua.lume" \
         --version "$VERSION" \
         --install-location "/" \
         --sign "$CERT_INSTALLER_NAME" \
         ./.release/lume.pkg; then
    log "error" "Failed to build installer package"
    exit 1
fi

# Verify the package was created
if [ ! -f "./.release/lume.pkg" ]; then
    log "error" "Package file ./.release/lume.pkg was not created"
    exit 1
fi

log "essential" "Package created successfully"

# --- Notarize ---
log "essential" "Submitting for notarization..."
if [ "$LOG_LEVEL" = "minimal" ] || [ "$LOG_LEVEL" = "none" ]; then
  # Minimal output - capture ID but hide details
  NOTARY_OUTPUT=$(xcrun notarytool submit ./.release/lume.pkg \
      --apple-id "${APPLE_ID}" \
      --team-id "${TEAM_ID}" \
      --password "${APP_SPECIFIC_PASSWORD}" \
      --wait 2>&1)

  # Check if notarization was successful
  if echo "$NOTARY_OUTPUT" | grep -q "status: Accepted"; then
    log "essential" "Notarization successful!"
  else
    log "error" "Notarization failed. Please check logs."
    log "error" "Notarization output:"
    echo "$NOTARY_OUTPUT"
    # Extract submission ID and fetch detailed log
    SUBMISSION_ID=$(echo "$NOTARY_OUTPUT" | grep "id:" | head -1 | awk '{print $2}')
    if [ -n "$SUBMISSION_ID" ]; then
      log "error" "Fetching notarization log for submission $SUBMISSION_ID..."
      xcrun notarytool log "$SUBMISSION_ID" \
          --apple-id "${APPLE_ID}" \
          --team-id "${TEAM_ID}" \
          --password "${APP_SPECIFIC_PASSWORD}" \
          developer_log.json 2>&1 || true
      if [ -f developer_log.json ]; then
        log "error" "Notarization log:"
        cat developer_log.json
      fi
    fi
    exit 1
  fi
else
  # Normal verbose output
  if ! xcrun notarytool submit ./.release/lume.pkg \
      --apple-id "${APPLE_ID}" \
      --team-id "${TEAM_ID}" \
      --password "${APP_SPECIFIC_PASSWORD}" \
      --wait; then
    log "error" "Notarization failed"
    # Try to fetch the log for the last submission
    LAST_ID=$(xcrun notarytool history \
        --apple-id "${APPLE_ID}" \
        --team-id "${TEAM_ID}" \
        --password "${APP_SPECIFIC_PASSWORD}" 2>&1 | grep "id:" | head -1 | awk '{print $2}')
    if [ -n "$LAST_ID" ]; then
      log "error" "Fetching notarization log for submission $LAST_ID..."
      xcrun notarytool log "$LAST_ID" \
          --apple-id "${APPLE_ID}" \
          --team-id "${TEAM_ID}" \
          --password "${APP_SPECIFIC_PASSWORD}" \
          developer_log.json 2>&1 || true
      if [ -f developer_log.json ]; then
        log "error" "Notarization log:"
        cat developer_log.json
      fi
    fi
    exit 1
  fi
fi

# Staple the notarization ticket to the .pkg
log "essential" "Stapling notarization ticket to .pkg..."
if ! xcrun stapler staple ./.release/lume.pkg > /dev/null 2>&1; then
  log "error" "Failed to staple notarization ticket to .pkg"
  exit 1
fi

# Staple the notarization ticket to the .app bundle
log "essential" "Stapling notarization ticket to .app bundle..."
if ! xcrun stapler staple "$APP_BUNDLE" > /dev/null 2>&1; then
  log "normal" "Note: Could not staple .app bundle directly (this is expected when notarizing via .pkg)"
fi

# --- Create release archives ---

# Get architecture and create OS identifier
ARCH=$(uname -m)
OS_IDENTIFIER="darwin-${ARCH}"
RELEASE_DIR="$(cd .release && pwd)"

log "essential" "Creating archives in $RELEASE_DIR..."
cd "$RELEASE_DIR"

# Clean up any existing artifacts first to avoid conflicts
rm -f lume-*.tar.gz lume-*.pkg.tar.gz

# Create a backward-compatible wrapper script at the tarball root
cat > lume <<'WRAPPER_EOF'
#!/bin/sh
exec "$(dirname "$0")/lume.app/Contents/MacOS/lume" "$@"
WRAPPER_EOF
chmod +x lume

# Create version-specific archives
log "essential" "Creating version-specific archives (${VERSION})..."

# Package the .app bundle and wrapper script
tar -czf "lume-${VERSION}-${OS_IDENTIFIER}.tar.gz" lume lume.app > /dev/null 2>&1

# Package the installer
tar -czf "lume-${VERSION}-${OS_IDENTIFIER}.pkg.tar.gz" lume.pkg > /dev/null 2>&1

# Create sha256 checksum file
log "essential" "Generating checksums..."
shasum -a 256 lume-*.tar.gz > checksums.txt
log "essential" "Package created successfully with checksums generated."

# Show what's in the release directory
log "essential" "Files in release directory:"
ls -la "$RELEASE_DIR"

# Ensure correct permissions
chmod 644 "$RELEASE_DIR"/*.tar.gz "$RELEASE_DIR"/*.pkg.tar.gz "$RELEASE_DIR"/checksums.txt

# Clean up
rm -rf "$TEMP_ROOT"

log "essential" "Build and packaging completed successfully."

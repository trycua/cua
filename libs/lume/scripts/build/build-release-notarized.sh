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

# Sign the binary with hardened runtime entitlements
log "essential" "Signing binary with entitlements..."
codesign --force --options runtime \
         --entitlement ./resources/lume.entitlements \
         --sign "$CERT_APPLICATION_NAME" \
         .build/release/lume 2> /dev/null

# Create a temporary directory for packaging
TEMP_ROOT=$(mktemp -d)
mkdir -p "$TEMP_ROOT/usr/local/bin"
cp -f .build/release/lume "$TEMP_ROOT/usr/local/bin/"

# Build the installer package
log "essential" "Building installer package..."
if ! pkgbuild --root "$TEMP_ROOT" \
         --identifier "com.trycua.lume" \
         --version "1.0" \
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

# Submit for notarization using stored credentials
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
    exit 1
  fi
fi

# Staple the notarization ticket
log "essential" "Stapling notarization ticket..."
if ! xcrun stapler staple ./.release/lume.pkg > /dev/null 2>&1; then
  log "error" "Failed to staple notarization ticket"
  exit 1
fi

# Create temporary directory for package extraction
EXTRACT_ROOT=$(mktemp -d)
PKG_PATH="$(pwd)/.release/lume.pkg"

# Extract the pkg using xar
cd "$EXTRACT_ROOT"
xar -xf "$PKG_PATH" > /dev/null 2>&1

# Verify Payload exists before proceeding
if [ ! -f "Payload" ]; then
    log "error" "Error: Payload file not found after xar extraction"
    exit 1
fi

# Create a directory for the extracted contents
mkdir -p extracted
cd extracted

# Extract the Payload
cat ../Payload | gunzip -dc | cpio -i > /dev/null 2>&1

# Verify the binary exists
if [ ! -f "usr/local/bin/lume" ]; then
    log "error" "Error: lume binary not found in expected location"
    exit 1
fi

# Get the release directory absolute path
RELEASE_DIR="$(realpath "$(dirname "$PKG_PATH")")"
log "normal" "Using release directory: $RELEASE_DIR"

# Copy extracted lume to the release directory
cp -f usr/local/bin/lume "$RELEASE_DIR/lume"

# Install to user-local bin directory (standard location)
USER_BIN="$HOME/.local/bin"
mkdir -p "$USER_BIN"
cp -f "$RELEASE_DIR/lume" "$USER_BIN/lume"

# Advise user to add to PATH if not present
if ! echo "$PATH" | grep -q "$USER_BIN"; then
  log "normal" "[lume build] Note: $USER_BIN is not in your PATH. Add 'export PATH=\"$USER_BIN:\$PATH\"' to your shell profile."
fi

# Get architecture and create OS identifier
ARCH=$(uname -m)
OS_IDENTIFIER="darwin-${ARCH}"

# Create versioned archives of the package with OS identifier in the name
log "essential" "Creating archives in $RELEASE_DIR..."
cd "$RELEASE_DIR"

# Clean up any existing artifacts first to avoid conflicts
rm -f lume-*.tar.gz lume-*.pkg.tar.gz

# Create version-specific archives
log "essential" "Creating version-specific archives (${VERSION})..."
# Package the binary
tar -czf "lume-${VERSION}-${OS_IDENTIFIER}.tar.gz" lume > /dev/null 2>&1
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
rm -rf "$EXTRACT_ROOT"

log "essential" "Build and packaging completed successfully."

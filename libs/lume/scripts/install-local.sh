#!/bin/bash
set -e

# Lume Local Installer
#
# Builds Lume from this checkout and installs it as `lume-local`, next to the
# released `lume` and sharing nothing with it:
#
#                   released Lume                 lume-local
#   command         ~/.local/bin/lume             ~/.local/bin/lume-local
#   files           ~/.local/share/lume           ~/.local/share/lume-local
#   LaunchAgent     com.trycua.lume_daemon        com.trycua.lume_daemon.local
#   port            7777                          7787
#   home and VMs    ~/.lume                       ~/.lume-local
#   release channel ~/.lume/release-channel       ~/.lume-local/release-channel
#   daemon logs     /tmp/lume_daemon.log          /tmp/lume_daemon.local.log
#
# This script never writes a path in the left column. The released Lume is
# installed by install.sh, which this script does not call or change.
#
# Environment (all optional):
#   LUME_LOCAL_HOME         home and VM directory (default: ~/.lume-local)
#   LUME_LOCAL_INSTALL_DIR  where the `lume-local` command goes
#                           (default: ~/.local/bin)
#   LUME_LOCAL_LOG_DIR      where the daemon logs go (default: /tmp)
# LUME_HOME is deliberately ignored: it may point at the released Lume's home.
# The `lume-local` command sets LUME_HOME to the home above, overriding any value
# it inherits, so the CLI and the daemon always use the same home.
#
# Every path is canonicalised (`.`, `..` and symlinks resolved) and refused if it
# equals, sits inside, or contains a path the released Lume owns
# (scripts/_lume-local-common.sh).

# Get the directory where this script is located
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# Navigate to the lume root directory (one level up from scripts/)
LUME_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

# Define colors for output
BOLD=$(tput bold 2>/dev/null || echo "")
NORMAL=$(tput sgr0 2>/dev/null || echo "")
RED=$(tput setaf 1 2>/dev/null || echo "")
GREEN=$(tput setaf 2 2>/dev/null || echo "")
BLUE=$(tput setaf 4 2>/dev/null || echo "")
YELLOW=$(tput setaf 3 2>/dev/null || echo "")

# The path guard shared with uninstall-local.sh
. "$SCRIPT_DIR/_lume-local-common.sh"

# Check if running as root or with sudo
if [ "$(id -u)" -eq 0 ] || [ -n "$SUDO_USER" ]; then
  echo "${RED}Error: Do not run this script with sudo or as root.${NORMAL}"
  exit 1
fi

# The names that make this install Local. They match the lume_* names in
# CuaChannel (libs/cua/crates/cua-home/src/channel.rs) and LumeChannel
# (src/FileSystem/LumeChannel.swift).
LOCAL_BIN_NAME="lume-local"
SERVICE_NAME="com.trycua.lume_daemon.local"
BUNDLE_ID="com.trycua.lume.local"
STABLE_PORT=7777
LOCAL_PORT=7787

# Where the `lume-local` command goes (user-specific, doesn't require sudo)
DEFAULT_INSTALL_DIR="$HOME/.local/bin"
INSTALL_DIR="${LUME_LOCAL_INSTALL_DIR:-$DEFAULT_INSTALL_DIR}"

# Where the binary and its resources go. A private directory: the resource
# bundle must sit next to the binary, and ~/.local/bin/lume_lume.bundle belongs
# to the released Lume.
APP_INSTALL_DIR="$HOME/.local/share/lume-local"

# Home and VM directory
LUME_LOCAL_HOME="${LUME_LOCAL_HOME:-$HOME/.lume-local}"

# Daemon log directory
LOG_DIR="${LUME_LOCAL_LOG_DIR:-/tmp}"

# Build configuration (debug or release)
BUILD_CONFIG="debug"

# Entitlement profile to use for local ad-hoc signing.
# Default excludes com.apple.vm.networking because macOS kills ad-hoc binaries
# carrying that restricted entitlement.
USE_BRIDGED_ENTITLEMENT=false

# Option to skip background service setup (default: install it)
INSTALL_BACKGROUND_SERVICE=true

# Port for lume-local serve
LUME_PORT="$LOCAL_PORT"

# Track whether we're using the .app bundle format
USE_APP_BUNDLE=false

# Ad-hoc signatures can become invalid when stale provenance metadata is
# carried across rebuilds. Clear extended attributes before signing.
clear_signature_metadata() {
  xattr -c "$1" 2>/dev/null || true
}

# Parse command line arguments
while [ "$#" -gt 0 ]; do
  case "$1" in
    --install-dir)
      INSTALL_DIR="$2"
      shift
      ;;
    --port)
      LUME_PORT="$2"
      shift
      ;;
    --release)
      BUILD_CONFIG="release"
      ;;
    --bridged-entitlement)
      USE_BRIDGED_ENTITLEMENT=true
      ;;
    --no-background-service)
      INSTALL_BACKGROUND_SERVICE=false
      ;;
    --help)
      echo "${BOLD}${BLUE}Lume Local Installer${NORMAL}"
      echo "Builds Lume from source and installs it as lume-local, beside the released lume."
      echo "Usage: $0 [OPTIONS]"
      echo ""
      echo "Options:"
      echo "  --install-dir DIR         Put the lume-local command in DIR (default: $DEFAULT_INSTALL_DIR)"
      echo "  --port PORT               Specify the port for lume-local serve (default: $LOCAL_PORT)"
      echo "  --release                 Build release configuration instead of debug"
      echo "  --bridged-entitlement     Sign with com.apple.vm.networking entitlement (requires proper Apple-approved signing)"
      echo "  --no-background-service   Do not setup the lume-local background service (LaunchAgent)"
      echo "  --help                    Display this help message"
      echo ""
      echo "Installs: $DEFAULT_INSTALL_DIR/lume-local, $APP_INSTALL_DIR,"
      echo "          LaunchAgent $SERVICE_NAME, home and VMs in $LUME_LOCAL_HOME"
      echo "Never touches: ~/.lume, ~/.local/bin/lume, ~/.local/share/lume, com.trycua.lume_daemon"
      echo ""
      echo "Examples:"
      echo "  $0                           # Build debug and install lume-local"
      echo "  $0 --release                 # Build release and install"
      echo "  $0 --port 7788               # Use port 7788 for the daemon"
      echo "  libs/lume/scripts/uninstall-local.sh   # Remove lume-local again"
      exit 0
      ;;
    *)
      echo "${RED}Unknown option: $1${NORMAL}"
      echo "Use --help for usage information"
      exit 1
      ;;
  esac
  shift
done

# Refuse settings that would reach the released Lume.
case "$LUME_PORT" in
  ''|*[!0-9]*)
    echo "${RED}Error: --port must be a number, got '$LUME_PORT'.${NORMAL}" >&2
    exit 2
    ;;
esac
if [ "$LUME_PORT" -lt 1 ] || [ "$LUME_PORT" -gt 65535 ]; then
  echo "${RED}Error: --port must be between 1 and 65535, got '$LUME_PORT'.${NORMAL}" >&2
  exit 2
fi
if [ "$LUME_PORT" -eq "$STABLE_PORT" ]; then
  echo "${RED}Error: port $STABLE_PORT belongs to the released Lume. lume-local uses $LOCAL_PORT.${NORMAL}" >&2
  exit 2
fi

# Canonicalise every path this script writes, and refuse any that reaches the
# released Lume. From here on these are the canonical forms.
LUME_LOCAL_HOME="$(lume_local_check "LUME_LOCAL_HOME" "$LUME_LOCAL_HOME")" || exit 2
case "$INSTALL_DIR" in
  /*) ;;
  *) echo "${RED}Error: --install-dir must be an absolute path, got '$INSTALL_DIR'.${NORMAL}" >&2; exit 2 ;;
esac
LOCAL_BIN="$(lume_local_check "the lume-local command ($INSTALL_DIR/$LOCAL_BIN_NAME)" "$INSTALL_DIR/$LOCAL_BIN_NAME")" || exit 2
# The command's own directory (an existing symlink named lume-local is replaced,
# not followed).
INSTALL_DIR="$(lume_local_canon "$INSTALL_DIR")" || exit 2
LOCAL_BIN="$INSTALL_DIR/$LOCAL_BIN_NAME"
APP_INSTALL_DIR="$(lume_local_check "the lume-local files directory" "$APP_INSTALL_DIR")" || exit 2
PLIST_PATH="$(lume_local_check "the lume-local LaunchAgent plist" "$HOME/Library/LaunchAgents/$SERVICE_NAME.plist")" || exit 2
LOG_FILE="$(lume_local_check "the lume-local daemon log" "$LOG_DIR/lume_daemon.local.log")" || exit 2
ERROR_LOG_FILE="$(lume_local_check "the lume-local daemon error log" "$LOG_DIR/lume_daemon.local.error.log")" || exit 2
WRAPPER_MARKER="# lume-local wrapper, managed by libs/lume/scripts/install-local.sh"

echo "${BOLD}${BLUE}"
echo "  ⠀⣀⣀⡀⠀⠀⠀⠀⢀⣀⣀⣀⡀⠘⠋⢉⠙⣷⠀⠀ ⠀"
echo " ⠀⠀⢀⣴⣿⡿⠋⣉⠁⣠⣾⣿⣿⣿⣿⡿⠿⣦⡈⠀⣿⡇⠃⠀"
echo " ⠀⠀⠀⣽⣿⣧⠀⠃⢰⣿⣿⡏⠙⣿⠿⢧⣀⣼⣷⠀⡿⠃⠀⠀"
echo " ⠀⠀⠀⠉⣿⣿⣦⠀⢿⣿⣿⣷⣾⡏⠀⠀⢹⣿⣿⠀⠀⠀⠀⠀⠀"
echo " ⠀⠀⠀⠀⠀⠉⠛⠁⠈⠿⣿⣿⣿⣷⣄⣠⡼⠟⠁${NORMAL}${BOLD}  Lume Local Installer${NORMAL}"
echo "${BLUE}           Build from source, installed as $LOCAL_BIN_NAME${NORMAL}"
echo ""
echo "Building ${BOLD}$BUILD_CONFIG${NORMAL} configuration from ${BOLD}$LUME_DIR${NORMAL}"
echo ""

# Check for required tools
if ! command -v swift &> /dev/null; then
  echo "${RED}Error: Swift is required but not installed.${NORMAL}"
  echo "Install Xcode Command Line Tools: xcode-select --install"
  exit 1
fi

# Build lume. The product is called `lume`; it is installed as `lume-local`.
build_lume() {
  echo "Building lume ($BUILD_CONFIG)..."
  cd "$LUME_DIR"

  if [ "$BUILD_CONFIG" = "release" ]; then
    swift build -c release --product lume
    BUILD_PATH="$LUME_DIR/.build/release"
  else
    swift build --product lume
    BUILD_PATH="$LUME_DIR/.build/debug"
  fi

  # Stage the installable files under their final names, so what is signed and
  # launch-checked is exactly what gets installed.
  STAGE_DIR="$BUILD_PATH/lume-local-stage"
  rm -rf "$STAGE_DIR"
  mkdir -p "$STAGE_DIR"

  # Use local-safe entitlements by default.
  ENTITLEMENTS_FILE="$LUME_DIR/resources/lume.local.entitlements"
  if [ "$USE_BRIDGED_ENTITLEMENT" = true ]; then
    ENTITLEMENTS_FILE="$LUME_DIR/resources/lume.entitlements"
  fi

  if [ "$USE_BRIDGED_ENTITLEMENT" = true ]; then
    # Assemble .app bundle for provisioning profile support
    echo "Assembling .app bundle for bridged networking..."

    APP_BUNDLE="$STAGE_DIR/$LOCAL_BIN_NAME.app"
    mkdir -p "$APP_BUNDLE/Contents/MacOS"

    cp -f "$BUILD_PATH/lume" "$APP_BUNDLE/Contents/MacOS/$LOCAL_BIN_NAME"

    # Copy resource bundle to Contents/Resources/.
    # It CANNOT go in Contents/MacOS/ (breaks codesign: "bundle format unrecognized")
    # and CANNOT go at the .app root (breaks codesign: "unsealed contents").
    # The Swift code uses Bundle.lumeResources which checks resourceURL first.
    mkdir -p "$APP_BUNDLE/Contents/Resources"
    if [ -d "$BUILD_PATH/lume_lume.bundle" ]; then
      cp -rf "$BUILD_PATH/lume_lume.bundle" "$APP_BUNDLE/Contents/Resources/"
    fi
    cp -f "$LUME_DIR/resources/AppIcon.icns" "$APP_BUNDLE/Contents/Resources/AppIcon.icns"

    # Stamp Info.plist with the version and the Local identity. A distinct
    # bundle id keeps macOS from treating this app as the released Lume.
    CURRENT_VERSION=$("$BUILD_PATH/lume" --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' || echo "0.0.0")
    sed "s/__VERSION__/$CURRENT_VERSION/g" "$LUME_DIR/resources/Info.plist" > "$APP_BUNDLE/Contents/Info.plist"
    /usr/libexec/PlistBuddy \
      -c "Set :CFBundleIdentifier $BUNDLE_ID" \
      -c "Set :CFBundleExecutable $LOCAL_BIN_NAME" \
      -c "Set :CFBundleName Lume Local" \
      -c "Set :CFBundleDisplayName Lume Local" \
      "$APP_BUNDLE/Contents/Info.plist"

    # Embed provisioning profile if available
    if [ -f "$LUME_DIR/resources/embedded.provisionprofile" ]; then
      cp "$LUME_DIR/resources/embedded.provisionprofile" "$APP_BUNDLE/Contents/embedded.provisionprofile"
    else
      echo "${YELLOW}Warning: No provisioning profile found at $LUME_DIR/resources/embedded.provisionprofile${NORMAL}"
      echo "${YELLOW}Bridged networking requires a provisioning profile from Apple Developer portal for $BUNDLE_ID.${NORMAL}"
    fi

    # Sign the bundle
    clear_signature_metadata "$APP_BUNDLE/Contents/MacOS/$LOCAL_BIN_NAME"
    codesign --force --entitlements "$ENTITLEMENTS_FILE" --sign - "$APP_BUNDLE/Contents/MacOS/$LOCAL_BIN_NAME"
    codesign --force --sign - "$APP_BUNDLE"

    # Verify the signed binary can launch from the bundle
    if "$APP_BUNDLE/Contents/MacOS/$LOCAL_BIN_NAME" --version >/dev/null 2>&1; then
      USE_APP_BUNDLE=true
    else
      echo "${YELLOW}Warning: binary did not launch from .app bundle with bridged entitlement; falling back to standalone binary.${NORMAL}"
      ENTITLEMENTS_FILE="$LUME_DIR/resources/lume.local.entitlements"
      rm -rf "$APP_BUNDLE"
      USE_APP_BUNDLE=false
    fi
  fi

  if [ "$USE_APP_BUNDLE" = false ]; then
    # Standard standalone binary (no .app bundle needed). The resource bundle
    # sits next to it, where Bundle.lumeResources looks.
    cp -f "$BUILD_PATH/lume" "$STAGE_DIR/$LOCAL_BIN_NAME"
    if [ -d "$BUILD_PATH/lume_lume.bundle" ]; then
      cp -rf "$BUILD_PATH/lume_lume.bundle" "$STAGE_DIR/"
    fi

    clear_signature_metadata "$STAGE_DIR/$LOCAL_BIN_NAME"
    codesign --force --identifier "$BUNDLE_ID" --entitlements "$ENTITLEMENTS_FILE" --sign - "$STAGE_DIR/$LOCAL_BIN_NAME"

    # Verify the signed binary can launch
    if ! "$STAGE_DIR/$LOCAL_BIN_NAME" --version >/dev/null 2>&1; then
      echo "${RED}Error: signed binary could not launch; refusing to install it.${NORMAL}"
      exit 1
    fi
  fi

  echo "${GREEN}Build complete!${NORMAL}"
}

# Install the binary
install_binary() {
  echo "Installing to $APP_INSTALL_DIR and $INSTALL_DIR..."

  mkdir -p "$INSTALL_DIR" "$APP_INSTALL_DIR"

  # Replace the files, not just their contents: a new inode keeps taskgated
  # from reusing a stale signature cache.
  rm -rf "$APP_INSTALL_DIR/$LOCAL_BIN_NAME" \
    "$APP_INSTALL_DIR/$LOCAL_BIN_NAME.app" \
    "$APP_INSTALL_DIR/lume_lume.bundle"

  if [ "$USE_APP_BUNDLE" = true ]; then
    cp -R "$STAGE_DIR/$LOCAL_BIN_NAME.app" "$APP_INSTALL_DIR/"
    REAL_BIN="$APP_INSTALL_DIR/$LOCAL_BIN_NAME.app/Contents/MacOS/$LOCAL_BIN_NAME"
  else
    cp -f "$STAGE_DIR/$LOCAL_BIN_NAME" "$APP_INSTALL_DIR/$LOCAL_BIN_NAME"
    chmod +x "$APP_INSTALL_DIR/$LOCAL_BIN_NAME"
    if [ -d "$STAGE_DIR/lume_lume.bundle" ]; then
      cp -rf "$STAGE_DIR/lume_lume.bundle" "$APP_INSTALL_DIR/"
    fi
    REAL_BIN="$APP_INSTALL_DIR/$LOCAL_BIN_NAME"
  fi

  # The command is a small wrapper, so nothing but one file lands in a bin
  # directory the released Lume shares.
  # It sets LUME_HOME to the validated Local home, overriding whatever the
  # caller exported (often the released Lume's), so every command uses the same
  # home as the daemon, including with a custom LUME_LOCAL_HOME.
  rm -f "$LOCAL_BIN"
  {
    printf '#!/bin/sh\n'
    printf '%s\n' "$WRAPPER_MARKER"
    printf 'LUME_HOME=%q\n' "$LUME_LOCAL_HOME"
    printf 'export LUME_HOME\n'
    printf 'exec %q "$@"\n' "$REAL_BIN"
  } > "$LOCAL_BIN"
  chmod +x "$LOCAL_BIN"

  echo "${GREEN}Installation complete!${NORMAL}"
  echo "lume-local installed to ${BOLD}$APP_INSTALL_DIR${NORMAL}"
  echo "CLI available at ${BOLD}$LOCAL_BIN${NORMAL}"

  # Check if the installation directory is in PATH
  if [ -n "${PATH##*$INSTALL_DIR*}" ]; then
    echo ""
    echo "${YELLOW}Note: $INSTALL_DIR is not in your PATH${NORMAL}"
    echo "Add this to your shell profile:"
    echo "  export PATH=\"\$PATH:$INSTALL_DIR\""
  fi
}

# Create the home and its own release-channel file
setup_home() {
  mkdir -p "$LUME_LOCAL_HOME"
  if [ ! -f "$LUME_LOCAL_HOME/release-channel" ]; then
    channel_tmp="$LUME_LOCAL_HOME/.release-channel.$$"
    printf '%s\n' "stable" > "$channel_tmp"
    mv -f "$channel_tmp" "$LUME_LOCAL_HOME/release-channel"
  fi
}

# Setup background service (LaunchAgent)
setup_background_service() {
  echo ""
  echo "Setting up LaunchAgent to run lume-local daemon..."

  # Create LaunchAgents directory if it doesn't exist
  mkdir -p "$HOME/Library/LaunchAgents"

  # Unload existing service if present
  if [ -f "$PLIST_PATH" ]; then
    echo "Existing LaunchAgent found. Unloading..."
    launchctl unload "$PLIST_PATH" 2>/dev/null || true
  fi

  mkdir -p "$(dirname "$LOG_FILE")" "$(dirname "$ERROR_LOG_FILE")"

  # Create the plist file
  cat <<EOF > "$PLIST_PATH"
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$SERVICE_NAME</string>
    <key>ProgramArguments</key>
    <array>
        <string>$LOCAL_BIN</string>
        <string>serve</string>
        <string>--port</string>
        <string>$LUME_PORT</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>WorkingDirectory</key>
    <string>$HOME</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:$HOME/.local/bin</string>
        <key>HOME</key>
        <string>$HOME</string>
        <key>LUME_HOME</key>
        <string>$LUME_LOCAL_HOME</string>
    </dict>
    <key>StandardOutPath</key>
    <string>$LOG_FILE</string>
    <key>StandardErrorPath</key>
    <string>$ERROR_LOG_FILE</string>
    <key>ProcessType</key>
    <string>Interactive</string>
    <key>SessionType</key>
    <string>Aqua</string>
</dict>
</plist>
EOF

  # Set permissions
  chmod 644 "$PLIST_PATH"
  touch "$LOG_FILE" "$ERROR_LOG_FILE"
  chmod 644 "$LOG_FILE" "$ERROR_LOG_FILE"

  # Load the LaunchAgent
  echo "Loading LaunchAgent..."
  launchctl load "$PLIST_PATH"

  echo "${GREEN}lume-local daemon LaunchAgent installed and loaded!${NORMAL}"
  echo "To check status: launchctl list | grep $SERVICE_NAME"
  echo "To view logs: tail -f $LOG_FILE"
}

# Remove background service
remove_background_service() {
  if [ -f "$PLIST_PATH" ]; then
    echo "Removing existing lume-local background service (LaunchAgent)..."
    launchctl unload "$PLIST_PATH" 2>/dev/null || true
    rm "$PLIST_PATH"
    echo "lume-local background service removed."
  fi
}

# Main installation flow
main() {
  build_lume
  install_binary
  setup_home

  echo ""
  echo "${GREEN}${BOLD}lume-local ($BUILD_CONFIG) has been successfully installed!${NORMAL}"
  echo "Run ${BOLD}$LOCAL_BIN_NAME${NORMAL} to get started. VMs and settings live in ${BOLD}$LUME_LOCAL_HOME${NORMAL}."
  echo "The released ${BOLD}lume${NORMAL} (port $STABLE_PORT, ~/.lume) is untouched."
  if [ "$USE_BRIDGED_ENTITLEMENT" = false ]; then
    echo "${YELLOW}Note: local installer signs without com.apple.vm.networking by default.${NORMAL}"
  fi

  if [ "$INSTALL_BACKGROUND_SERVICE" = true ]; then
    setup_background_service
  else
    remove_background_service
    echo "Skipping background service setup (use without --no-background-service to enable)."
  fi

  echo ""
  echo "${YELLOW}Note: This is a local build. Auto-updater is not installed, and $LOCAL_BIN_NAME never updates itself.${NORMAL}"
  echo "To remove it: ${BOLD}$SCRIPT_DIR/uninstall-local.sh${NORMAL}"
}

# Run the installation
main

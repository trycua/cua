#!/bin/bash
set -e

# Lume Local/Debug Installer
# This script builds and installs Lume from the local source code

# Get the directory where this script is located
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# Navigate to the lume root directory (one level up from scripts/)
LUME_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

# Define colors for output
BOLD=$(tput bold)
NORMAL=$(tput sgr0)
RED=$(tput setaf 1)
GREEN=$(tput setaf 2)
BLUE=$(tput setaf 4)
YELLOW=$(tput setaf 3)

# Check if running as root or with sudo
if [ "$(id -u)" -eq 0 ] || [ -n "$SUDO_USER" ]; then
  echo "${RED}Error: Do not run this script with sudo or as root.${NORMAL}"
  exit 1
fi

# Default installation directory (user-specific, doesn't require sudo)
DEFAULT_INSTALL_DIR="$HOME/.local/bin"
INSTALL_DIR="${INSTALL_DIR:-$DEFAULT_INSTALL_DIR}"

# Build configuration (debug or release)
BUILD_CONFIG="debug"

# Option to skip background service setup (default: install it)
INSTALL_BACKGROUND_SERVICE=true

# Default port for lume serve (default: 7777)
LUME_PORT=7777

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
    --no-background-service)
      INSTALL_BACKGROUND_SERVICE=false
      ;;
    --help)
      echo "${BOLD}${BLUE}Lume Local Installer${NORMAL}"
      echo "Usage: $0 [OPTIONS]"
      echo ""
      echo "Options:"
      echo "  --install-dir DIR         Install to the specified directory (default: $DEFAULT_INSTALL_DIR)"
      echo "  --port PORT               Specify the port for lume serve (default: 7777)"
      echo "  --release                 Build release configuration instead of debug"
      echo "  --no-background-service   Do not setup the Lume background service (LaunchAgent)"
      echo "  --help                    Display this help message"
      echo ""
      echo "Examples:"
      echo "  $0                           # Build debug and install to $DEFAULT_INSTALL_DIR"
      echo "  $0 --release                 # Build release and install"
      echo "  $0 --port 7778               # Use port 7778 for the daemon"
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

echo "${BOLD}${BLUE}"
echo "  ⠀⣀⣀⡀⠀⠀⠀⠀⢀⣀⣀⣀⡀⠘⠋⢉⠙⣷⠀⠀ ⠀"
echo " ⠀⠀⢀⣴⣿⡿⠋⣉⠁⣠⣾⣿⣿⣿⣿⡿⠿⣦⡈⠀⣿⡇⠃⠀"
echo " ⠀⠀⠀⣽⣿⣧⠀⠃⢰⣿⣿⡏⠙⣿⠿⢧⣀⣼⣷⠀⡿⠃⠀⠀"
echo " ⠀⠀⠀⠉⣿⣿⣦⠀⢿⣿⣿⣷⣾⡏⠀⠀⢹⣿⣿⠀⠀⠀⠀⠀⠀"
echo " ⠀⠀⠀⠀⠀⠉⠛⠁⠈⠿⣿⣿⣿⣷⣄⣠⡼⠟⠁${NORMAL}${BOLD}  Lume Local Installer${NORMAL}"
echo "${BLUE}           Build from source${NORMAL}"
echo ""
echo "Building ${BOLD}$BUILD_CONFIG${NORMAL} configuration from ${BOLD}$LUME_DIR${NORMAL}"
echo ""

# Check for required tools
if ! command -v swift &> /dev/null; then
  echo "${RED}Error: Swift is required but not installed.${NORMAL}"
  echo "Install Xcode Command Line Tools: xcode-select --install"
  exit 1
fi

# Build lume
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

  # Codesign the binary
  echo "Codesigning binary..."
  codesign --force --entitlement "$LUME_DIR/resources/lume.entitlements" --sign - "$BUILD_PATH/lume"

  echo "${GREEN}Build complete!${NORMAL}"
}

# Install the binary
install_binary() {
  echo "Installing to $INSTALL_DIR..."

  # Create install directory if it doesn't exist
  mkdir -p "$INSTALL_DIR"

  # Copy the binary
  cp -f "$BUILD_PATH/lume" "$INSTALL_DIR/lume"
  chmod +x "$INSTALL_DIR/lume"

  # Copy the resource bundle if it exists (contains unattended presets)
  if [ -d "$BUILD_PATH/lume_lume.bundle" ]; then
    rm -rf "$INSTALL_DIR/lume_lume.bundle"
    cp -rf "$BUILD_PATH/lume_lume.bundle" "$INSTALL_DIR/"
    echo "Resource bundle installed to ${BOLD}$INSTALL_DIR/lume_lume.bundle${NORMAL}"
  fi

  echo "${GREEN}Installation complete!${NORMAL}"
  echo "Lume has been installed to ${BOLD}$INSTALL_DIR/lume${NORMAL}"

  # Check if the installation directory is in PATH
  if [ -n "${PATH##*$INSTALL_DIR*}" ]; then
    echo ""
    echo "${YELLOW}Note: $INSTALL_DIR is not in your PATH${NORMAL}"
    echo "Add this to your shell profile:"
    echo "  export PATH=\"\$PATH:$INSTALL_DIR\""
  fi
}

# Setup background service (LaunchAgent)
setup_background_service() {
  SERVICE_NAME="com.trycua.lume_daemon"
  PLIST_PATH="$HOME/Library/LaunchAgents/$SERVICE_NAME.plist"
  LUME_BIN="$INSTALL_DIR/lume"

  echo ""
  echo "Setting up LaunchAgent to run lume daemon..."

  # Create LaunchAgents directory if it doesn't exist
  mkdir -p "$HOME/Library/LaunchAgents"

  # Unload existing service if present
  if [ -f "$PLIST_PATH" ]; then
    echo "Existing LaunchAgent found. Unloading..."
    launchctl unload "$PLIST_PATH" 2>/dev/null || true
  fi

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
        <string>$LUME_BIN</string>
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
    </dict>
    <key>StandardOutPath</key>
    <string>/tmp/lume_daemon.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/lume_daemon.error.log</string>
    <key>ProcessType</key>
    <string>Interactive</string>
    <key>SessionType</key>
    <string>Aqua</string>
</dict>
</plist>
EOF

  # Set permissions
  chmod 644 "$PLIST_PATH"
  touch /tmp/lume_daemon.log /tmp/lume_daemon.error.log
  chmod 644 /tmp/lume_daemon.log /tmp/lume_daemon.error.log

  # Load the LaunchAgent
  echo "Loading LaunchAgent..."
  launchctl load "$PLIST_PATH"

  echo "${GREEN}Lume daemon LaunchAgent installed and loaded!${NORMAL}"
  echo "To check status: launchctl list | grep $SERVICE_NAME"
  echo "To view logs: tail -f /tmp/lume_daemon.log"
}

# Remove background service
remove_background_service() {
  SERVICE_NAME="com.trycua.lume_daemon"
  PLIST_PATH="$HOME/Library/LaunchAgents/$SERVICE_NAME.plist"

  if [ -f "$PLIST_PATH" ]; then
    echo "Removing existing Lume background service (LaunchAgent)..."
    launchctl unload "$PLIST_PATH" 2>/dev/null || true
    rm "$PLIST_PATH"
    echo "Lume background service removed."
  fi
}

# Main installation flow
main() {
  build_lume
  install_binary

  echo ""
  echo "${GREEN}${BOLD}Lume ($BUILD_CONFIG) has been successfully installed!${NORMAL}"
  echo "Run ${BOLD}lume${NORMAL} to get started."

  if [ "$INSTALL_BACKGROUND_SERVICE" = true ]; then
    setup_background_service
  else
    remove_background_service
    echo "Skipping background service setup (use without --no-background-service to enable)."
  fi

  echo ""
  echo "${YELLOW}Note: This is a local build. Auto-updater is not installed.${NORMAL}"
}

# Run the installation
main

#!/bin/bash
set -e

# Lume Installer
# This script installs Lume to your system

# Define colors for output
BOLD=$(tput bold)
NORMAL=$(tput sgr0)
RED=$(tput setaf 1)
GREEN=$(tput setaf 2)
BLUE=$(tput setaf 4)
YELLOW=$(tput setaf 3)

# Default installation directory (user-specific, doesn't require sudo)
DEFAULT_INSTALL_DIR="$HOME/.local/bin"
INSTALL_DIR="${INSTALL_DIR:-$DEFAULT_INSTALL_DIR}"

# GitHub info
GITHUB_REPO="trycua/cua"
LATEST_RELEASE_URL="https://api.github.com/repos/$GITHUB_REPO/releases/latest"

# Option to skip background service setup (default: install it)
INSTALL_BACKGROUND_SERVICE=true

# Parse command line arguments
while [ "$#" -gt 0 ]; do
  case "$1" in
    --install-dir=*)
      INSTALL_DIR="${1#*=}"
      ;;
    --no-background-service|--skip-background-service)
      INSTALL_BACKGROUND_SERVICE=false
      ;;
    --help)
      echo "${BOLD}${BLUE}Lume Installer${NORMAL}"
      echo "Usage: $0 [OPTIONS]"
      echo ""
      echo "Options:"
      echo "  --install-dir=DIR         Install to the specified directory (default: $DEFAULT_INSTALL_DIR)"
      echo "  --no-background-service   Do not setup the Lume background service (LaunchAgent)"
      echo "  --help                    Display this help message"
      echo ""
      echo "Examples:"
      echo "  $0                                   # Install to $DEFAULT_INSTALL_DIR and setup background service"
      echo "  $0 --install-dir=/usr/local/bin      # Install to system directory (may require root privileges)"
      echo "  $0 --no-background-service           # Install without setting up the background service"
      echo "  INSTALL_DIR=/opt/lume $0             # Install to /opt/lume (legacy env var support)"
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

echo "${BOLD}${BLUE}Lume Installer${NORMAL}"
echo "This script will install Lume to your system."

# Check if we're running with appropriate permissions
check_permissions() {
  # System directories that typically require root privileges
  SYSTEM_DIRS=("/usr/local/bin" "/usr/bin" "/bin" "/opt")
  
  NEEDS_ROOT=false
  for DIR in "${SYSTEM_DIRS[@]}"; do
    if [[ "$INSTALL_DIR" == "$DIR"* ]] && [ ! -w "$INSTALL_DIR" ]; then
      NEEDS_ROOT=true
      break
    fi
  done
  
  if [ "$NEEDS_ROOT" = true ]; then
    echo "${YELLOW}Warning: Installing to $INSTALL_DIR may require root privileges.${NORMAL}"
    echo "Consider these alternatives:"
    echo "  • Install to a user-writable location: $0 --install-dir=$HOME/.local/bin"
    echo "  • Create the directory with correct permissions first:"
    echo "    sudo mkdir -p $INSTALL_DIR && sudo chown $(whoami) $INSTALL_DIR"
    echo ""
    
    # Check if we already have write permission (might have been set up previously)
    if [ ! -w "$INSTALL_DIR" ] && [ ! -w "$(dirname "$INSTALL_DIR")" ]; then
      echo "${RED}Error: You don't have write permission to $INSTALL_DIR${NORMAL}"
      echo "Please choose a different installation directory or ensure you have the proper permissions."
      exit 1
    fi
  fi
}

# Detect OS and architecture
detect_platform() {
  OS=$(uname -s | tr '[:upper:]' '[:lower:]')
  ARCH=$(uname -m)
  
  if [ "$OS" != "darwin" ]; then
    echo "${RED}Error: Currently only macOS is supported.${NORMAL}"
    exit 1
  fi
  
  if [ "$ARCH" != "arm64" ]; then
    echo "${RED}Error: Lume only supports macOS on Apple Silicon (ARM64).${NORMAL}"
    exit 1
  fi
  
  PLATFORM="darwin-arm64"
  echo "Detected platform: ${BOLD}$PLATFORM${NORMAL}"
}

# Create temporary directory
create_temp_dir() {
  TEMP_DIR=$(mktemp -d)
  echo "Using temporary directory: $TEMP_DIR"
  
  # Make sure we clean up on exit
  trap 'rm -rf "$TEMP_DIR"' EXIT
}

# Download the latest release
download_release() {
  echo "Downloading latest Lume release..."
  
  # Use the direct download link with the non-versioned symlink
  DOWNLOAD_URL="https://github.com/$GITHUB_REPO/releases/latest/download/lume.tar.gz"
  echo "Downloading from: $DOWNLOAD_URL"
  
  # Download the tarball
  if command -v curl &> /dev/null; then
    curl -L --progress-bar "$DOWNLOAD_URL" -o "$TEMP_DIR/lume.tar.gz"
    
    # Verify the download was successful
    if [ ! -s "$TEMP_DIR/lume.tar.gz" ]; then
      echo "${RED}Error: Failed to download Lume.${NORMAL}"
      echo "The download URL may be incorrect or the file may not exist."
      exit 1
    fi
    
    # Verify the file is a valid archive
    if ! tar -tzf "$TEMP_DIR/lume.tar.gz" > /dev/null 2>&1; then
      echo "${RED}Error: The downloaded file is not a valid tar.gz archive.${NORMAL}"
      echo "Let's try the alternative URL..."
      
      # Try alternative URL
      ALT_DOWNLOAD_URL="https://github.com/$GITHUB_REPO/releases/latest/download/lume-$PLATFORM.tar.gz"
      echo "Downloading from alternative URL: $ALT_DOWNLOAD_URL"
      curl -L --progress-bar "$ALT_DOWNLOAD_URL" -o "$TEMP_DIR/lume.tar.gz"
      
      # Check again
      if ! tar -tzf "$TEMP_DIR/lume.tar.gz" > /dev/null 2>&1; then
        echo "${RED}Error: Could not download a valid Lume archive.${NORMAL}"
        echo "Please try installing Lume manually from: https://github.com/$GITHUB_REPO/releases/latest"
        exit 1
      fi
    fi
  else
    echo "${RED}Error: curl is required but not installed.${NORMAL}"
    exit 1
  fi
}

# Extract and install
install_binary() {
  echo "Extracting archive..."
  tar -xzf "$TEMP_DIR/lume.tar.gz" -C "$TEMP_DIR"
  
  echo "Installing to $INSTALL_DIR..."
  
  # Create install directory if it doesn't exist
  mkdir -p "$INSTALL_DIR"
  
  # Move the binary to the installation directory
  mv "$TEMP_DIR/lume" "$INSTALL_DIR/"
  
  # Make the binary executable
  chmod +x "$INSTALL_DIR/lume"
  
  echo "${GREEN}Installation complete!${NORMAL}"
  echo "Lume has been installed to ${BOLD}$INSTALL_DIR/lume${NORMAL}"
  
  # Check if the installation directory is in PATH
  if [ -n "${PATH##*$INSTALL_DIR*}" ]; then
    SHELL_NAME=$(basename "$SHELL")
    echo "${YELLOW}Warning: $INSTALL_DIR is not in your PATH.${NORMAL}"
    case "$SHELL_NAME" in
      zsh)
        echo "To add it, run:"
        echo "  echo 'export PATH=\"\$PATH:$INSTALL_DIR\"' >> ~/.zprofile"
        ;;
      bash)
        echo "To add it, run:"
        echo "  echo 'export PATH=\"\$PATH:$INSTALL_DIR\"' >> ~/.bash_profile"
        ;;
      fish)
        echo "To add it, run:"
        echo "  echo 'fish_add_path $INSTALL_DIR' >> ~/.config/fish/config.fish"
        ;;
      *)
        echo "Add $INSTALL_DIR to your PATH in your shell profile file."
        ;;
    esac
  fi
}

# Main installation flow
main() {
  check_permissions
  detect_platform
  create_temp_dir
  download_release
  install_binary

  echo ""
  echo "${GREEN}${BOLD}Lume has been successfully installed!${NORMAL}"
  echo "Run ${BOLD}lume${NORMAL} to get started."

  if [ "$INSTALL_BACKGROUND_SERVICE" = true ]; then
    # --- Setup background service (LaunchAgent) for Lume ---
    SERVICE_NAME="com.trycua.lume_daemon"
    PLIST_PATH="$HOME/Library/LaunchAgents/$SERVICE_NAME.plist"
    LUME_BIN="$INSTALL_DIR/lume"

    echo ""
    echo "Setting up LaunchAgent to run lume daemon on login..."

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
    launchctl unload "$PLIST_PATH" 2>/dev/null || true
    launchctl load "$PLIST_PATH"

    echo "${GREEN}Lume daemon LaunchAgent installed and loaded. It will start automatically on login!${NORMAL}"
    echo "To check status: launchctl list | grep $SERVICE_NAME"
    echo "To view logs: tail -f /tmp/lume_daemon.log"
    echo ""
    echo "To remove the lume daemon service, run:"
    echo "  launchctl unload \"$PLIST_PATH\""
    echo "  rm \"$PLIST_PATH\""
  else
    SERVICE_NAME="com.trycua.lume_daemon"
    PLIST_PATH="$HOME/Library/LaunchAgents/$SERVICE_NAME.plist"
    if [ -f "$PLIST_PATH" ]; then
      echo "Removing existing Lume background service (LaunchAgent)..."
      launchctl unload "$PLIST_PATH" 2>/dev/null || true
      rm "$PLIST_PATH"
      echo "Lume background service (LaunchAgent) removed."
    else
      echo "Skipping Lume background service (LaunchAgent) setup as requested (use --no-background-service)."
    fi
  fi
}

# Run the installation
main

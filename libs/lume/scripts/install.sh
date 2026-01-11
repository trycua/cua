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

# Check if running as root or with sudo
if [ "$(id -u)" -eq 0 ] || [ -n "$SUDO_USER" ]; then
  echo "${RED}Error: Do not run this script with sudo or as root.${NORMAL}"
  echo "If you need to install to a system directory, create it first with proper permissions:"
  echo "  sudo mkdir -p /desired/directory && sudo chown $(whoami) /desired/directory"
  echo "Then run the installer normally:"
  echo "  ./install.sh --install-dir=/desired/directory"
  exit 1
fi

# Default installation directory (user-specific, doesn't require sudo)
DEFAULT_INSTALL_DIR="$HOME/.local/bin"
INSTALL_DIR="${INSTALL_DIR:-$DEFAULT_INSTALL_DIR}"

# GitHub info
GITHUB_REPO="trycua/cua"
LATEST_RELEASE_URL="https://api.github.com/repos/$GITHUB_REPO/releases/latest"

# Option to skip background service setup (default: install it)
INSTALL_BACKGROUND_SERVICE=true

# Option to skip auto-updater setup (default: install it)
INSTALL_AUTO_UPDATER=true

# Option to run updater at login (default: false, uses cron instead)
UPDATE_ON_LOGIN=false

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
    --no-background-service)
      INSTALL_BACKGROUND_SERVICE=false
      ;;
    --no-auto-updater)
      INSTALL_AUTO_UPDATER=false
      ;;
    --update-on-login)
      UPDATE_ON_LOGIN=true
      ;;
    --help)
      echo "${BOLD}${BLUE}Lume Installer${NORMAL}"
      echo "Usage: $0 [OPTIONS]"
      echo ""
      echo "Options:"
      echo "  --install-dir DIR         Install to the specified directory (default: $DEFAULT_INSTALL_DIR)"
      echo "  --port PORT               Specify the port for lume serve (default: 7777)"
      echo "  --no-background-service   Do not setup the Lume background service (LaunchAgent)"
      echo "  --no-auto-updater         Do not setup automatic updates"
      echo "  --update-on-login         Check for updates at login (adds second Login Item)"
      echo "  --help                    Display this help message"
      echo ""
      echo "Examples:"
      echo "  $0                                   # Install to $DEFAULT_INSTALL_DIR and setup background service"
      echo "  $0 --install-dir=/usr/local/bin      # Install to system directory (may require root privileges)"
      echo "  $0 --port 7778                       # Use port 7778 instead of the default 7777"
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

# Get the latest lume release tag
get_latest_lume_tag() {
  echo "Finding latest Lume release..." >&2

  local page=1
  local per_page=100
  local max_pages=10  # Safety limit (1000 tags max)
  local LUME_TAG=""

  while [ $page -le $max_pages ]; do
    echo "Checking page $page..." >&2

    local response=$(curl -s "https://api.github.com/repos/$GITHUB_REPO/tags?per_page=$per_page&page=$page")

    if [ -z "$response" ] || [ "$(echo "$response" | grep -c '"name":')" -eq 0 ]; then
      if [ $page -eq 1 ]; then
        echo "${RED}Error: Failed to fetch tags from GitHub API.${NORMAL}" >&2
        exit 1
      else
        echo "${RED}Error: No lume tags found after checking $((page - 1)) pages.${NORMAL}" >&2
        exit 1
      fi
    fi

    LUME_TAG=$(echo "$response" \
      | grep '"name": "lume-' \
      | head -n 1 \
      | cut -d '"' -f 4)

    if [ -n "$LUME_TAG" ]; then
      echo "Found latest Lume release: ${BOLD}$LUME_TAG${NORMAL}" >&2
      echo "$LUME_TAG"
      return 0
    fi

    page=$((page + 1))
  done

  echo "${RED}Error: Could not find any lume tags after checking $max_pages pages.${NORMAL}" >&2
  exit 1
}

# Download the latest release
download_release() {
  LUME_TAG=$(get_latest_lume_tag)

  if [ -z "$LUME_TAG" ]; then
    echo "${RED}Error: Could not determine latest Lume release tag.${NORMAL}"
    exit 1
  fi

  echo "Downloading Lume release $LUME_TAG..."

  # Use the direct download link with the lume release tag
  DOWNLOAD_URL="https://github.com/$GITHUB_REPO/releases/download/$LUME_TAG/lume.tar.gz"
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

      # Try alternative URL with platform-specific name
      ALT_DOWNLOAD_URL="https://github.com/$GITHUB_REPO/releases/download/$LUME_TAG/lume-darwin.tar.gz"
      echo "Downloading from alternative URL: $ALT_DOWNLOAD_URL"
      curl -L --progress-bar "$ALT_DOWNLOAD_URL" -o "$TEMP_DIR/lume.tar.gz"

      # Check again
      if ! tar -tzf "$TEMP_DIR/lume.tar.gz" > /dev/null 2>&1; then
        echo "${RED}Error: Could not download a valid Lume archive.${NORMAL}"
        echo "Please try installing Lume manually from: https://github.com/$GITHUB_REPO/releases/tag/$LUME_TAG"
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

# Install the auto-updater script
install_updater_script() {
  UPDATER_SCRIPT="$INSTALL_DIR/lume-update"

  echo "Installing auto-updater script to $UPDATER_SCRIPT..."

  cat <<'UPDATER_EOF' > "$UPDATER_SCRIPT"
#!/bin/bash
# Lume Auto-Updater
# This script checks for updates and installs them if available

set -e

LOG_FILE="/tmp/lume_updater.log"
GITHUB_REPO="trycua/cua"

log() {
  echo "[$(date '+%Y-%m-%d %H:%M:%S')] $1" >> "$LOG_FILE"
}

log "Starting Lume update check..."

# Find lume binary location
LUME_BIN=$(command -v lume 2>/dev/null || echo "$HOME/.local/bin/lume")
INSTALL_DIR=$(dirname "$LUME_BIN")

if [ ! -x "$LUME_BIN" ]; then
  log "ERROR: lume binary not found at $LUME_BIN"
  exit 1
fi

# Get current version
CURRENT_VERSION=$("$LUME_BIN" --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1 || echo "0.0.0")
log "Current version: $CURRENT_VERSION"

# Get latest release tag from GitHub
get_latest_tag() {
  local page=1
  local per_page=100
  local max_pages=5

  while [ $page -le $max_pages ]; do
    local response=$(curl -s "https://api.github.com/repos/$GITHUB_REPO/tags?per_page=$per_page&page=$page")

    if [ -z "$response" ]; then
      return 1
    fi

    local tag=$(echo "$response" | grep '"name": "lume-' | head -n 1 | cut -d '"' -f 4)

    if [ -n "$tag" ]; then
      echo "$tag"
      return 0
    fi

    page=$((page + 1))
  done

  return 1
}

LATEST_TAG=$(get_latest_tag)
if [ -z "$LATEST_TAG" ]; then
  log "ERROR: Could not fetch latest release tag"
  exit 1
fi

# Extract version from tag (lume-vX.Y.Z -> X.Y.Z)
LATEST_VERSION=$(echo "$LATEST_TAG" | sed 's/lume-v//' | sed 's/lume-//')
log "Latest version: $LATEST_VERSION"

# Compare versions (simple string comparison works for semver)
version_gt() {
  [ "$(printf '%s\n' "$1" "$2" | sort -V | tail -n1)" = "$1" ] && [ "$1" != "$2" ]
}

apply_update() {
  log "Downloading and applying update..."

  TEMP_DIR=$(mktemp -d)
  trap 'rm -rf "$TEMP_DIR"' EXIT

  DOWNLOAD_URL="https://github.com/$GITHUB_REPO/releases/download/$LATEST_TAG/lume.tar.gz"

  if curl -sL "$DOWNLOAD_URL" -o "$TEMP_DIR/lume.tar.gz"; then
    if tar -tzf "$TEMP_DIR/lume.tar.gz" > /dev/null 2>&1; then
      tar -xzf "$TEMP_DIR/lume.tar.gz" -C "$TEMP_DIR"

      # Stop the daemon before updating
      launchctl unload "$HOME/Library/LaunchAgents/com.trycua.lume_daemon.plist" 2>/dev/null || true

      # Install new binary
      mv "$TEMP_DIR/lume" "$INSTALL_DIR/"
      chmod +x "$INSTALL_DIR/lume"

      # Restart the daemon
      launchctl load "$HOME/Library/LaunchAgents/com.trycua.lume_daemon.plist" 2>/dev/null || true

      log "Successfully updated lume to version $LATEST_VERSION"

      # Show macOS notification
      osascript -e "display notification \"Updated to version $LATEST_VERSION\" with title \"Lume Updated\"" 2>/dev/null || true
    else
      log "ERROR: Downloaded file is not a valid archive"
      exit 1
    fi
  else
    log "ERROR: Failed to download update"
    exit 1
  fi
}

if version_gt "$LATEST_VERSION" "$CURRENT_VERSION"; then
  log "New version available: $LATEST_VERSION (current: $CURRENT_VERSION)"

  # Check if --apply flag was passed (for manual/scripted updates)
  if [ "${1:-}" = "--apply" ]; then
    apply_update
  else
    # Show dialog asking user if they want to update
    RESPONSE=$(osascript -e "display dialog \"Lume $LATEST_VERSION is available (current: $CURRENT_VERSION).\" buttons {\"Later\", \"Update Now\"} default button \"Update Now\" with title \"Lume Update\"" 2>/dev/null || echo "")

    if echo "$RESPONSE" | grep -q "Update Now"; then
      log "User chose to update"
      apply_update
    else
      log "User chose to skip update"
    fi
  fi
else
  log "Already up to date (version $CURRENT_VERSION)"
fi

log "Update check complete"
UPDATER_EOF

  chmod +x "$UPDATER_SCRIPT"
  echo "Auto-updater script installed."
}

# Remove legacy auto-updater LaunchAgent if it exists (now integrated into daemon)
cleanup_legacy_updater() {
  UPDATER_SERVICE_NAME="com.trycua.lume_updater"
  UPDATER_PLIST_PATH="$HOME/Library/LaunchAgents/$UPDATER_SERVICE_NAME.plist"

  if [ -f "$UPDATER_PLIST_PATH" ]; then
    echo "Removing legacy auto-updater LaunchAgent (now integrated into daemon)..."
    launchctl unload "$UPDATER_PLIST_PATH" 2>/dev/null || true
    rm "$UPDATER_PLIST_PATH"
  fi
}

# Remove auto-updater if it exists
remove_auto_updater() {
  UPDATER_SERVICE_NAME="com.trycua.lume_updater"
  UPDATER_PLIST_PATH="$HOME/Library/LaunchAgents/$UPDATER_SERVICE_NAME.plist"
  UPDATER_SCRIPT="$INSTALL_DIR/lume-update"

  if [ -f "$UPDATER_PLIST_PATH" ]; then
    echo "Removing existing auto-updater LaunchAgent..."
    launchctl unload "$UPDATER_PLIST_PATH" 2>/dev/null || true
    rm "$UPDATER_PLIST_PATH"
  fi

  if [ -f "$UPDATER_SCRIPT" ]; then
    rm "$UPDATER_SCRIPT"
  fi

  echo "Auto-updater removed."
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
    WRAPPER_SCRIPT="$INSTALL_DIR/lume-daemon"
    UPDATER_SCRIPT="$INSTALL_DIR/lume-update"

    echo ""
    echo "Setting up LaunchAgent to run lume daemon on login..."

    # Create LaunchAgents directory if it doesn't exist
    mkdir -p "$HOME/Library/LaunchAgents"

    # Unload existing service if present
    if [ -f "$PLIST_PATH" ]; then
      echo "Existing LaunchAgent found. Unloading..."
      launchctl unload "$PLIST_PATH" 2>/dev/null || true
    fi

    # Clean up old wrapper script if it exists (no longer needed)
    if [ -f "$WRAPPER_SCRIPT" ]; then
      rm -f "$WRAPPER_SCRIPT"
    fi

    # Create the plist file - runs signed lume binary directly (no wrapper)
    # This ensures proper code signing identity shows in Login Items
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

  # Install updater script and setup update checks
  if [ "$INSTALL_AUTO_UPDATER" = true ]; then
    install_updater_script
    UPDATER_SCRIPT="$INSTALL_DIR/lume-update"
    UPDATER_SERVICE_NAME="com.trycua.lume_updater"
    UPDATER_PLIST_PATH="$HOME/Library/LaunchAgents/$UPDATER_SERVICE_NAME.plist"

    if [ "$UPDATE_ON_LOGIN" = true ]; then
      # Remove cron job if switching to login-based updates
      crontab -l 2>/dev/null | grep -v "lume-update" | crontab - 2>/dev/null || true

      # Unload existing updater service if present
      if [ -f "$UPDATER_PLIST_PATH" ]; then
        launchctl unload "$UPDATER_PLIST_PATH" 2>/dev/null || true
      fi

      # Create LaunchAgent that runs updater at login (not persistent)
      cat <<EOF > "$UPDATER_PLIST_PATH"
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$UPDATER_SERVICE_NAME</string>
    <key>ProgramArguments</key>
    <array>
        <string>$UPDATER_SCRIPT</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/tmp/lume_updater.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/lume_updater.error.log</string>
</dict>
</plist>
EOF
      chmod 644 "$UPDATER_PLIST_PATH"
      launchctl load "$UPDATER_PLIST_PATH"
      echo "${GREEN}Auto-updater installed. Checks at login.${NORMAL}"
    else
      # Clean up any LaunchAgent updater (we use cron by default)
      cleanup_legacy_updater

      # Setup cron job for daily update check (doesn't show in Login Items)
      CRON_ENTRY="0 10 * * * $UPDATER_SCRIPT >/tmp/lume_updater.log 2>&1"
      EXISTING_CRON=$(crontab -l 2>/dev/null || echo "")
      NEW_CRON=$(echo "$EXISTING_CRON" | grep -v "lume-update" || echo "")
      echo "${NEW_CRON}${NEW_CRON:+
}${CRON_ENTRY}" | crontab -
      echo "${GREEN}Auto-updater installed. Checks daily at 10am via cron.${NORMAL}"
    fi

    # Run updater once after installation
    if [ -x "$UPDATER_SCRIPT" ]; then
      "$UPDATER_SCRIPT" &
    fi
  else
    # Remove updater cron job and LaunchAgent if auto-updater is disabled
    crontab -l 2>/dev/null | grep -v "lume-update" | crontab - 2>/dev/null || true
    cleanup_legacy_updater
  fi
}

# Run the installation
main

#!/bin/bash
set -e

# Lume Installer
# This script installs Lume to your system

# Define colors for output
BOLD=$(tput bold 2>/dev/null || echo "")
NORMAL=$(tput sgr0 2>/dev/null || echo "")
RED=$(tput setaf 1 2>/dev/null || echo "")
GREEN=$(tput setaf 2 2>/dev/null || echo "")
BLUE=$(tput setaf 4 2>/dev/null || echo "")
YELLOW=$(tput setaf 3 2>/dev/null || echo "")

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

# Default .app bundle installation directory
APP_INSTALL_DIR="$HOME/.local/share/lume"

# GitHub info
GITHUB_REPO="trycua/cua"
LATEST_RELEASE_URL="https://api.github.com/repos/$GITHUB_REPO/releases/latest"

# Option to skip background service setup (default: install it)
INSTALL_BACKGROUND_SERVICE=true

# Deprecated auto-updater flags are still accepted for compatibility, but
# scheduled self-updates are no longer installed. Updates are explicit:
# `lume check-update` and `lume update --apply`.
INSTALL_AUTO_UPDATER=false
UPDATE_ON_LOGIN=false

# Release selection:
#   LUME_VERSION=0.3.10 pins a specific `lume-v0.3.10` release.
#   LUME_BAKED_VERSION is updated in the release PR so the default
#   installer path does not need a GitHub API call.
LUME_VERSION="${LUME_VERSION:-}"
# ~~~ BAKED_VERSION: auto-updated in the release PR — do not edit ~~~
LUME_BAKED_VERSION="0.3.16" # x-release-please-version
# ~~~ END_BAKED_VERSION ~~~

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
      echo "${YELLOW}Warning: --no-auto-updater is deprecated; scheduled auto-updates are no longer installed.${NORMAL}"
      INSTALL_AUTO_UPDATER=false
      ;;
    --update-on-login)
      echo "${YELLOW}Warning: --update-on-login is deprecated; use 'lume update --apply' for explicit updates.${NORMAL}"
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
      echo "  --no-auto-updater         Deprecated no-op; scheduled auto-updates are no longer installed"
      echo "  --update-on-login         Deprecated no-op; use 'lume update --apply'"
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

echo "${BOLD}${BLUE}"
echo "  ⠀⣀⣀⡀⠀⠀⠀⠀⢀⣀⣀⣀⡀⠘⠋⢉⠙⣷⠀⠀ ⠀"
echo " ⠀⠀⢀⣴⣿⡿⠋⣉⠁⣠⣾⣿⣿⣿⣿⡿⠿⣦⡈⠀⣿⡇⠃⠀"
echo " ⠀⠀⠀⣽⣿⣧⠀⠃⢰⣿⣿⡏⠙⣿⠿⢧⣀⣼⣷⠀⡿⠃⠀⠀"
echo " ⠀⠀⠀⠉⣿⣿⣦⠀⢿⣿⣿⣷⣾⡏⠀⠀⢹⣿⣿⠀⠀⠀⠀⠀⠀"
echo " ⠀⠀⠀⠀⠀⠉⠛⠁⠈⠿⣿⣿⣿⣷⣄⣠⡼⠟⠁${NORMAL}${BOLD}  Lume Installer${NORMAL}"
echo "${BLUE}           macOS VM CLI and server${NORMAL}"
echo ""
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
  if [ -n "$LUME_VERSION" ]; then
    local pinned="${LUME_VERSION#lume-v}"
    pinned="${pinned#v}"
    echo "Using Lume release from LUME_VERSION: ${BOLD}lume-v$pinned${NORMAL}" >&2
    echo "lume-v$pinned"
    return 0
  fi

  if [ -n "$LUME_BAKED_VERSION" ]; then
    local baked="${LUME_BAKED_VERSION#v}"
    echo "Using baked Lume release: ${BOLD}lume-v$baked${NORMAL}" >&2
    echo "lume-v$baked"
    return 0
  fi

  echo "Finding latest Lume release..." >&2

  local page=1
  local per_page=100
  local max_pages=10  # Safety limit (1000 tags max)
  local LUME_TAG=""

  while [ $page -le $max_pages ]; do
    echo "Checking page $page..." >&2

    local response=$(curl -s "https://api.github.com/repos/$GITHUB_REPO/releases?per_page=$per_page&page=$page")

    if [ -z "$response" ] || [ "$(echo "$response" | grep -c '"tag_name"')" -eq 0 ]; then
      if [ $page -eq 1 ]; then
        echo "${RED}Error: Failed to fetch tags from GitHub API.${NORMAL}" >&2
        exit 1
      else
        echo "${RED}Error: No lume tags found after checking $((page - 1)) pages.${NORMAL}" >&2
        exit 1
      fi
    fi

    LUME_TAG=$(echo "$response" \
      | grep -oE '"tag_name":\s*"lume-[^"]*"' \
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

  # Create directories
  mkdir -p "$INSTALL_DIR"
  mkdir -p "$APP_INSTALL_DIR"

  if [ -d "$TEMP_DIR/lume.app" ]; then
    # --- New .app bundle format ---
    echo "Installing lume.app bundle..."

    # Remove old standalone binary and resource bundle if present (migration)
    if [ -f "$INSTALL_DIR/lume" ] && [ ! -L "$INSTALL_DIR/lume" ]; then
      # It's a regular file (old standalone binary), not a symlink or script
      if file "$INSTALL_DIR/lume" | grep -q "Mach-O"; then
        echo "Migrating from standalone binary to .app bundle..."
        rm -f "$INSTALL_DIR/lume"
      fi
    fi
    rm -rf "$INSTALL_DIR/lume_lume.bundle"

    # Install the .app bundle
    rm -rf "$APP_INSTALL_DIR/lume.app"
    mv "$TEMP_DIR/lume.app" "$APP_INSTALL_DIR/"

    # Create wrapper script so lume is callable from the command line
    cat > "$INSTALL_DIR/lume" <<WRAPPER_EOF
#!/bin/sh
exec "$APP_INSTALL_DIR/lume.app/Contents/MacOS/lume" "\$@"
WRAPPER_EOF
    chmod +x "$INSTALL_DIR/lume"

    echo "${GREEN}Installation complete!${NORMAL}"
    echo "Lume installed to ${BOLD}$APP_INSTALL_DIR/lume.app${NORMAL}"
    echo "CLI available at ${BOLD}$INSTALL_DIR/lume${NORMAL}"
  else
    # --- Legacy standalone binary format ---
    echo "Installing lume binary..."

    mv "$TEMP_DIR/lume" "$INSTALL_DIR/"
    chmod +x "$INSTALL_DIR/lume"

    # Move the resource bundle if it exists (contains unattended presets)
    if [ -d "$TEMP_DIR/lume_lume.bundle" ]; then
      rm -rf "$INSTALL_DIR/lume_lume.bundle"
      mv "$TEMP_DIR/lume_lume.bundle" "$INSTALL_DIR/"
      echo "Resource bundle installed to ${BOLD}$INSTALL_DIR/lume_lume.bundle${NORMAL}"
    fi

    echo "${GREEN}Installation complete!${NORMAL}"
    echo "Lume has been installed to ${BOLD}$INSTALL_DIR/lume${NORMAL}"
  fi

  # Check if the installation directory is in PATH
  if [ -n "${PATH##*$INSTALL_DIR*}" ]; then
    SHELL_NAME=$(basename "$SHELL")
    echo ""
    echo "${YELLOW}╔════════════════════════════════════════════════════════════════╗${NORMAL}"
    echo "${YELLOW}║  ${BOLD}ACTION REQUIRED:${NORMAL}${YELLOW} $INSTALL_DIR is not in your PATH${NORMAL}"
    echo "${YELLOW}╚════════════════════════════════════════════════════════════════╝${NORMAL}"
    echo ""
    case "$SHELL_NAME" in
      zsh)
        echo "Run these commands:"
        echo "  ${BOLD}echo 'export PATH=\"\$PATH:$INSTALL_DIR\"' >> ~/.zshrc${NORMAL}"
        echo "  ${BOLD}source ~/.zshrc${NORMAL}"
        echo ""
        echo "Or restart your terminal after running the first command."
        ;;
      bash)
        echo "Run these commands:"
        echo "  ${BOLD}echo 'export PATH=\"\$PATH:$INSTALL_DIR\"' >> ~/.bash_profile${NORMAL}"
        echo "  ${BOLD}source ~/.bash_profile${NORMAL}"
        echo ""
        echo "Or restart your terminal after running the first command."
        ;;
      fish)
        echo "Run this command:"
        echo "  ${BOLD}echo 'fish_add_path $INSTALL_DIR' >> ~/.config/fish/config.fish${NORMAL}"
        echo ""
        echo "Then restart your terminal."
        ;;
      *)
        echo "Add $INSTALL_DIR to your PATH in your shell profile file,"
        echo "then restart your terminal or source your profile."
        ;;
    esac
    echo ""
  fi
}

# Remove scheduled auto-updater artifacts from older installers.
cleanup_deprecated_auto_updater() {
  UPDATER_SERVICE_NAME="com.trycua.lume_updater"
  UPDATER_PLIST_PATH="$HOME/Library/LaunchAgents/$UPDATER_SERVICE_NAME.plist"
  UPDATER_SCRIPT="$INSTALL_DIR/lume-update"

  if crontab -l 2>/dev/null | grep -q "lume-update"; then
    echo "Removing deprecated auto-updater cron job..."
    crontab -l 2>/dev/null | grep -v "lume-update" | crontab - 2>/dev/null || true
  fi

  if [ -f "$UPDATER_PLIST_PATH" ]; then
    echo "Removing deprecated auto-updater LaunchAgent..."
    launchctl unload "$UPDATER_PLIST_PATH" 2>/dev/null || true
    rm "$UPDATER_PLIST_PATH"
  fi

  if [ -f "$UPDATER_SCRIPT" ]; then
    rm "$UPDATER_SCRIPT"
  fi
}

# Record consent-aware installation and per-version release events through
# the installed binary so identity, preference precedence, request timeouts,
# and 2xx-only marker semantics stay in one implementation. This happens
# before the LaunchAgent can start Lume and race first-run registration.
record_installation_telemetry() {
  echo ""
  echo "${BLUE}Telemetry defaults to enabled for new installations; saved preferences and environment overrides are honored.${NORMAL}"
  echo "${BLUE}When enabled, Lume collects a pseudonymous installation ID and bounded, content-free usage metadata.${NORMAL}"
  echo "  No prompts, VM/image names, file paths, or VM contents are collected."
  echo "  Disable persistently at any time: $INSTALL_DIR/lume config telemetry disable"

  INSTALL_CHANNEL="${LUME_INSTALL_CHANNEL:-install_script}"
  case "$INSTALL_CHANNEL" in
    install_script|update_apply|first_run) ;;
    *) INSTALL_CHANNEL="install_script" ;;
  esac

  # `--version` is a side-effect-free command after the registration preflight.
  # Telemetry failure must never turn a successful install into a failure.
  LUME_INSTALL_CHANNEL="$INSTALL_CHANNEL" \
    "$INSTALL_DIR/lume" --version >/dev/null 2>&1 || true
}

# Main installation flow
main() {
  check_permissions
  detect_platform
  create_temp_dir
  download_release
  install_binary
  record_installation_telemetry

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

    # Create the plist file - runs lume via the wrapper script
    # The wrapper delegates to lume.app/Contents/MacOS/lume
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

  cleanup_deprecated_auto_updater
  echo ""
  echo "Updates are explicit. To check later, run:"
  echo "  ${BOLD}lume check-update${NORMAL}"
  echo "To update, run:"
  echo "  ${BOLD}lume update --apply${NORMAL}"
}

# Run the installation
main

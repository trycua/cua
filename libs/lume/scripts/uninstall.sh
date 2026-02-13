#!/bin/bash
set -e

# Lume Uninstaller
# This script removes Lume from your system
# Usage: /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/lume/scripts/uninstall.sh)"

# Define colors for output
BOLD=$(tput bold 2>/dev/null || echo "")
NORMAL=$(tput sgr0 2>/dev/null || echo "")
RED=$(tput setaf 1 2>/dev/null || echo "")
GREEN=$(tput setaf 2 2>/dev/null || echo "")
YELLOW=$(tput setaf 3 2>/dev/null || echo "")
BLUE=$(tput setaf 4 2>/dev/null || echo "")

# Default options
PURGE_DATA=false
FORCE=false

# Parse command line arguments
while [ "$#" -gt 0 ]; do
  case "$1" in
    --purge)
      PURGE_DATA=true
      ;;
    --force|-f)
      FORCE=true
      ;;
    --help|-h)
      echo "${BOLD}${BLUE}Lume Uninstaller${NORMAL}"
      echo "Usage: $0 [OPTIONS]"
      echo ""
      echo "Options:"
      echo "  --purge     Also remove all Lume data (VMs, cache, config)"
      echo "  --force     Skip confirmation prompts"
      echo "  --help      Display this help message"
      echo ""
      echo "Examples:"
      echo "  $0                # Uninstall Lume, keep data"
      echo "  $0 --purge        # Uninstall Lume and remove all data"
      echo "  $0 --purge --force  # Uninstall without prompts"
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
echo " ⠀⠀⠀⠀⠀⠉⠛⠁⠈⠿⣿⣿⣿⣷⣄⣠⡼⠟⠁${NORMAL}${BOLD}  Lume Uninstaller${NORMAL}"
echo "${BLUE}           macOS VM CLI and server${NORMAL}"
echo ""

# Confirmation prompt
if [ "$FORCE" = false ]; then
  echo "This will uninstall Lume from your system."
  if [ "$PURGE_DATA" = true ]; then
    echo "${YELLOW}WARNING: --purge flag detected. This will also delete:${NORMAL}"
    echo "  - All VMs in ~/.lume/"
    echo "  - Configuration in ~/.config/lume/"
    echo "  - Cached images"
  fi
  echo ""
  read -p "Are you sure you want to continue? [y/N] " -n 1 -r
  echo ""
  if [[ ! $REPLY =~ ^[Yy]$ ]]; then
    echo "Uninstall cancelled."
    exit 0
  fi
fi

echo ""
echo "${BOLD}Stopping Lume services...${NORMAL}"

# Stop and remove LaunchAgent for daemon
DAEMON_PLIST="$HOME/Library/LaunchAgents/com.trycua.lume_daemon.plist"
if [ -f "$DAEMON_PLIST" ]; then
  echo "  Unloading lume daemon..."
  launchctl unload "$DAEMON_PLIST" 2>/dev/null || true
  rm -f "$DAEMON_PLIST"
  echo "  ${GREEN}Removed daemon LaunchAgent${NORMAL}"
else
  echo "  Daemon LaunchAgent not found (skipped)"
fi

# Stop and remove LaunchAgent for updater
UPDATER_PLIST="$HOME/Library/LaunchAgents/com.trycua.lume_updater.plist"
if [ -f "$UPDATER_PLIST" ]; then
  echo "  Unloading lume updater..."
  launchctl unload "$UPDATER_PLIST" 2>/dev/null || true
  rm -f "$UPDATER_PLIST"
  echo "  ${GREEN}Removed updater LaunchAgent${NORMAL}"
else
  echo "  Updater LaunchAgent not found (skipped)"
fi

# Remove cron job for auto-updater
if crontab -l 2>/dev/null | grep -q "lume-update"; then
  echo "  Removing cron job..."
  crontab -l 2>/dev/null | grep -v "lume-update" | crontab - 2>/dev/null || true
  echo "  ${GREEN}Removed auto-updater cron job${NORMAL}"
else
  echo "  Auto-updater cron job not found (skipped)"
fi

echo ""
echo "${BOLD}Removing Lume binaries...${NORMAL}"

# Find and remove lume binary
LUME_BIN=$(command -v lume 2>/dev/null || echo "")
if [ -z "$LUME_BIN" ]; then
  # Check common locations
  for loc in "$HOME/.local/bin/lume" "/usr/local/bin/lume" "/opt/lume/lume"; do
    if [ -f "$loc" ]; then
      LUME_BIN="$loc"
      break
    fi
  done
fi

if [ -n "$LUME_BIN" ] && [ -f "$LUME_BIN" ]; then
  INSTALL_DIR=$(dirname "$LUME_BIN")
  rm -f "$LUME_BIN"
  echo "  ${GREEN}Removed $LUME_BIN${NORMAL}"

  # Remove updater script if in same directory
  if [ -f "$INSTALL_DIR/lume-update" ]; then
    rm -f "$INSTALL_DIR/lume-update"
    echo "  ${GREEN}Removed $INSTALL_DIR/lume-update${NORMAL}"
  fi

  # Remove legacy wrapper script if exists
  if [ -f "$INSTALL_DIR/lume-daemon" ]; then
    rm -f "$INSTALL_DIR/lume-daemon"
    echo "  ${GREEN}Removed $INSTALL_DIR/lume-daemon${NORMAL}"
  fi

  # Remove legacy resource bundle if exists
  if [ -d "$INSTALL_DIR/lume_lume.bundle" ]; then
    rm -rf "$INSTALL_DIR/lume_lume.bundle"
    echo "  ${GREEN}Removed $INSTALL_DIR/lume_lume.bundle${NORMAL}"
  fi
else
  echo "  ${YELLOW}Lume binary not found (skipped)${NORMAL}"
fi

# Remove .app bundle if installed (new format)
APP_INSTALL_DIR="$HOME/.local/share/lume"
if [ -d "$APP_INSTALL_DIR/lume.app" ]; then
  rm -rf "$APP_INSTALL_DIR/lume.app"
  echo "  ${GREEN}Removed $APP_INSTALL_DIR/lume.app${NORMAL}"
  # Remove the share directory if empty
  rmdir "$APP_INSTALL_DIR" 2>/dev/null && echo "  ${GREEN}Removed $APP_INSTALL_DIR${NORMAL}" || true
fi

# Remove log files
echo ""
echo "${BOLD}Removing log files...${NORMAL}"
for logfile in /tmp/lume_daemon.log /tmp/lume_daemon.error.log /tmp/lume_updater.log /tmp/lume_updater.error.log; do
  if [ -f "$logfile" ]; then
    rm -f "$logfile"
    echo "  ${GREEN}Removed $logfile${NORMAL}"
  fi
done

# Purge data if requested
if [ "$PURGE_DATA" = true ]; then
  echo ""
  echo "${BOLD}${YELLOW}Purging Lume data...${NORMAL}"

  # Remove VMs and cache
  if [ -d "$HOME/.lume" ]; then
    # Stop any running VMs first
    if [ -n "$LUME_BIN" ] && [ -x "$LUME_BIN" ]; then
      echo "  Attempting to stop running VMs..."
      "$LUME_BIN" ls 2>/dev/null | grep -E "running|starting" | awk '{print $1}' | while read vm; do
        "$LUME_BIN" stop "$vm" 2>/dev/null || true
      done
    fi

    rm -rf "$HOME/.lume"
    echo "  ${GREEN}Removed ~/.lume/ (VMs and cache)${NORMAL}"
  else
    echo "  ~/.lume/ not found (skipped)"
  fi

  # Remove config
  if [ -d "$HOME/.config/lume" ]; then
    rm -rf "$HOME/.config/lume"
    echo "  ${GREEN}Removed ~/.config/lume/ (configuration)${NORMAL}"
  else
    echo "  ~/.config/lume/ not found (skipped)"
  fi
fi

echo ""
echo "${GREEN}${BOLD}Lume has been successfully uninstalled!${NORMAL}"
if [ "$PURGE_DATA" = false ]; then
  echo ""
  echo "Note: Your VMs and configuration were preserved."
  echo "To also remove all data, run with ${BOLD}--purge${NORMAL}:"
  echo "  /bin/bash -c \"\$(curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/lume/scripts/uninstall.sh)\" -- --purge"
fi
echo ""

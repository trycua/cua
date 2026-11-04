#!/bin/bash
set -e

# Lume Local Uninstaller
#
# Removes only what install-local.sh installed as `lume-local`:
#
#   ~/.local/bin/lume-local              the command
#   ~/.local/share/lume-local            the binary and its resources
#   ~/Library/LaunchAgents/com.trycua.lume_daemon.local.plist
#   /tmp/lume_daemon.local*.log          the daemon logs
#   ~/Library/Logs/lume-local            logs of detached runs (`run --detach`)
#   ~/.lume-local                        home: VMs, settings, cache (asks first)
#
# The released Lume (~/.local/bin/lume, ~/.local/share/lume, ~/.lume, the
# com.trycua.lume_daemon LaunchAgent) is never read or changed.
#
# Usage: uninstall-local.sh [--yes] [--keep-vms] [--install-dir DIR]
#
# Every path is canonicalised (`.`, `..` and symlinks resolved) and refused if it
# equals, sits inside, or contains a path the released Lume owns, so
# `--yes` can never delete the released Lume's VMs through an alias such as
# `$HOME/./.lume` or a symlink to it (scripts/_lume-local-common.sh).
#
# Environment (all optional; give the same values you gave install-local.sh):
#   LUME_LOCAL_HOME, LUME_LOCAL_INSTALL_DIR, LUME_LOCAL_LOG_DIR
# LUME_HOME is deliberately ignored: it may point at the released Lume's home.

# Get the directory where this script is located
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# Define colors for output
BOLD=$(tput bold 2>/dev/null || echo "")
NORMAL=$(tput sgr0 2>/dev/null || echo "")
RED=$(tput setaf 1 2>/dev/null || echo "")
GREEN=$(tput setaf 2 2>/dev/null || echo "")
YELLOW=$(tput setaf 3 2>/dev/null || echo "")
BLUE=$(tput setaf 4 2>/dev/null || echo "")

# The path guard shared with install-local.sh
. "$SCRIPT_DIR/_lume-local-common.sh"

LOCAL_BIN_NAME="lume-local"
SERVICE_NAME="com.trycua.lume_daemon.local"
WRAPPER_MARKER="# lume-local wrapper, managed by libs/lume/scripts/install-local.sh"

INSTALL_DIR="${LUME_LOCAL_INSTALL_DIR:-$HOME/.local/bin}"
APP_INSTALL_DIR="$HOME/.local/share/lume-local"
LUME_LOCAL_HOME="${LUME_LOCAL_HOME:-$HOME/.lume-local}"
LOG_DIR="${LUME_LOCAL_LOG_DIR:-/tmp}"

ASSUME_YES=false
KEEP_VMS=false

while [ "$#" -gt 0 ]; do
  case "$1" in
    --yes|-y)
      ASSUME_YES=true
      ;;
    --keep-vms)
      KEEP_VMS=true
      ;;
    --install-dir)
      INSTALL_DIR="$2"
      shift
      ;;
    --help|-h)
      echo "${BOLD}${BLUE}Lume Local Uninstaller${NORMAL}"
      echo "Removes lume-local. The released lume is never touched."
      echo "Usage: $0 [OPTIONS]"
      echo ""
      echo "Options:"
      echo "  --yes, -y         Answer yes to every question, including deleting the VMs in $LUME_LOCAL_HOME"
      echo "  --keep-vms        Never delete $LUME_LOCAL_HOME (VMs, settings, cache), even with --yes"
      echo "  --install-dir DIR Where the lume-local command was installed (default: $INSTALL_DIR)"
      echo "  --help            Display this help message"
      echo ""
      echo "Without --yes, it asks before removing lume-local, and again before deleting any VMs."
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

# Canonicalise every path this script deletes (`.`, `..` and symlinks resolved),
# and refuse any that equals, sits inside, or contains a path the released Lume
# owns. From here on these are the canonical forms.
LUME_LOCAL_HOME="$(lume_local_check "LUME_LOCAL_HOME" "$LUME_LOCAL_HOME")" || exit 2
lume_local_no_edge_space "--install-dir" "$INSTALL_DIR" || exit 2
lume_local_no_edge_space "LUME_LOCAL_LOG_DIR" "$LOG_DIR" || exit 2
case "$INSTALL_DIR" in
  /*) ;;
  *) echo "${RED}Error: --install-dir must be an absolute path, got '$INSTALL_DIR'.${NORMAL}" >&2; exit 2 ;;
esac
lume_local_check "the lume-local command ($INSTALL_DIR/$LOCAL_BIN_NAME)" "$INSTALL_DIR/$LOCAL_BIN_NAME" >/dev/null || exit 2
INSTALL_DIR="$(lume_local_canon "$INSTALL_DIR")" || exit 2
LOCAL_BIN="$INSTALL_DIR/$LOCAL_BIN_NAME"
APP_INSTALL_DIR="$(lume_local_check "the lume-local files directory" "$APP_INSTALL_DIR")" || exit 2
PLIST_PATH="$(lume_local_check "the lume-local LaunchAgent plist" "$HOME/Library/LaunchAgents/$SERVICE_NAME.plist")" || exit 2
LOG_FILE="$(lume_local_check "the lume-local daemon log" "$LOG_DIR/lume_daemon.local.log")" || exit 2
ERROR_LOG_FILE="$(lume_local_check "the lume-local daemon error log" "$LOG_DIR/lume_daemon.local.error.log")" || exit 2
RUN_LOG_DIR="$(lume_local_check "the lume-local run logs directory" "$HOME/Library/Logs/lume-local")" || exit 2

# Ask a yes/no question. Defaults to no. --yes answers yes.
confirm() {
  if [ "$ASSUME_YES" = true ]; then
    return 0
  fi
  local reply
  read -r -p "$1 [y/N] " reply || reply=""
  case "$reply" in
    y|Y|yes|YES) return 0 ;;
    *) return 1 ;;
  esac
}

# Whether $LOCAL_BIN is the wrapper install-local.sh wrote.
is_our_wrapper() {
  [ -f "$LOCAL_BIN" ] && [ ! -L "$LOCAL_BIN" ] && grep -qF "$WRAPPER_MARKER" "$LOCAL_BIN" 2>/dev/null
}

echo "${BOLD}${BLUE}Lume Local Uninstaller${NORMAL}"
echo ""
echo "This removes lume-local: the command, its files, its LaunchAgent ($SERVICE_NAME) and logs."
echo "The released lume, ~/.lume and port 7777 are not touched."
echo ""

if ! confirm "Remove lume-local?"; then
  echo "Uninstall cancelled."
  exit 0
fi

# Decide about the VMs now, so nothing is half done if the answer is no.
DELETE_HOME=false
if [ -d "$LUME_LOCAL_HOME" ]; then
  VM_COUNT=$(find "$LUME_LOCAL_HOME" -mindepth 2 -maxdepth 2 -name config.json 2>/dev/null | wc -l | tr -d ' ')
  echo ""
  echo "${BOLD}$LUME_LOCAL_HOME${NORMAL} holds $VM_COUNT VM(s), plus settings and cache."
  if [ "$KEEP_VMS" = true ]; then
    echo "Keeping it (--keep-vms)."
  elif confirm "Delete $LUME_LOCAL_HOME and everything in it, including the VMs?"; then
    DELETE_HOME=true
  else
    echo "Keeping it."
  fi
fi

echo ""
echo "${BOLD}Stopping lume-local services...${NORMAL}"

# Stop and remove the LaunchAgent. This also ends the daemon and the VMs it
# hosts.
if [ -f "$PLIST_PATH" ]; then
  echo "  Unloading lume-local daemon..."
  launchctl unload "$PLIST_PATH" 2>/dev/null || true
  rm -f "$PLIST_PATH"
  echo "  ${GREEN}Removed daemon LaunchAgent${NORMAL}"
else
  echo "  Daemon LaunchAgent not found (skipped)"
fi

# VMs started outside the daemon are still running; stop them before their
# files go.
if [ "$DELETE_HOME" = true ] && is_our_wrapper; then
  echo "  Attempting to stop running lume-local VMs..."
  "$LOCAL_BIN" ls 2>/dev/null | grep -E "running|starting" | awk '{print $1}' | while read -r vm; do
    "$LOCAL_BIN" stop "$vm" 2>/dev/null || true
  done
fi

echo ""
echo "${BOLD}Removing lume-local files...${NORMAL}"

if is_our_wrapper; then
  rm -f "$LOCAL_BIN"
  echo "  ${GREEN}Removed $LOCAL_BIN${NORMAL}"
elif [ -e "$LOCAL_BIN" ] || [ -L "$LOCAL_BIN" ]; then
  echo "  ${YELLOW}$LOCAL_BIN was not installed by install-local.sh (left in place)${NORMAL}"
else
  echo "  $LOCAL_BIN not found (skipped)"
fi

if [ -d "$APP_INSTALL_DIR" ]; then
  rm -rf "$APP_INSTALL_DIR"
  echo "  ${GREEN}Removed $APP_INSTALL_DIR${NORMAL}"
else
  echo "  $APP_INSTALL_DIR not found (skipped)"
fi

for logfile in "$LOG_FILE" "$ERROR_LOG_FILE"; do
  if [ -f "$logfile" ]; then
    rm -f "$logfile"
    echo "  ${GREEN}Removed $logfile${NORMAL}"
  fi
done

if [ -d "$RUN_LOG_DIR" ]; then
  rm -rf "$RUN_LOG_DIR"
  echo "  ${GREEN}Removed $RUN_LOG_DIR${NORMAL}"
fi

if [ "$DELETE_HOME" = true ]; then
  rm -rf "$LUME_LOCAL_HOME"
  echo "  ${GREEN}Removed $LUME_LOCAL_HOME (VMs, settings, cache)${NORMAL}"
fi

echo ""
echo "${GREEN}${BOLD}lume-local has been uninstalled.${NORMAL}"
if [ -d "$LUME_LOCAL_HOME" ]; then
  echo "Kept $LUME_LOCAL_HOME. Delete it yourself if you no longer need those VMs."
fi
echo ""

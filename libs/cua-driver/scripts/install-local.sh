#!/bin/bash
#
# cua-driver local/debug installer. Builds from the current source tree
# and installs the resulting CuaDriver.app + cua-driver CLI onto the
# developer's machine. Mirrors lume's scripts/install-local.sh shape.
#
# Installs to the same paths as scripts/install.sh (the production
# installer), so TCC grants made against one install survive the other:
#   app bundle  → /Applications/CuaDriver.app
#   CLI symlink → ~/.local/bin/cua-driver
#
# The script prompts for sudo only on the specific steps that need it
# (moving the .app into /Applications)
# — do NOT run the whole script with `sudo`.
#
# --release builds the release configuration (default is debug — faster).
# --daemon installs a LaunchAgent that runs `cua-driver serve` on login.
#
# Not for end-users (use scripts/install.sh for that — fetches signed +
# notarized release from GitHub).
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CUA_DRIVER_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

# Guard `tput` against environments where TERM is unset (agent sandboxes,
# CI containers, `launchd` jobs): tput aborts with "No value for $TERM and
# no -T specified" otherwise, killing the script before any work starts.
BOLD=$(tput bold 2>/dev/null || true)
NORMAL=$(tput sgr0 2>/dev/null || true)
RED=$(tput setaf 1 2>/dev/null || true)
GREEN=$(tput setaf 2 2>/dev/null || true)
BLUE=$(tput setaf 4 2>/dev/null || true)
YELLOW=$(tput setaf 3 2>/dev/null || true)

if [ "$(id -u)" -eq 0 ] || [ -n "${SUDO_USER:-}" ]; then
    echo "${RED}Error: do not run this script with sudo or as root.${NORMAL}"
    echo "The script will prompt for sudo on the specific operations that"
    echo "need it (writing to /Applications); running the"
    echo "whole thing as root puts the LaunchAgent plist under /var/root"
    echo "and breaks telemetry / config paths."
    exit 1
fi

# --- Parse arguments ----------------------------------------------------

BUILD_CONFIG="debug"
INSTALL_DAEMON=false     # LaunchAgent for `cua-driver serve`

while [ "$#" -gt 0 ]; do
    case "$1" in
        --release)
            BUILD_CONFIG="release"
            ;;
        --daemon)
            INSTALL_DAEMON=true
            ;;
        --help|-h)
            echo "${BOLD}${BLUE}cua-driver local installer${NORMAL}"
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --release    Build the release configuration (default: debug)."
            echo "  --daemon     Also install a LaunchAgent that runs 'cua-driver serve'"
            echo "               on login, so the per-pid AX cache is always available."
            echo "  --help       Show this help."
            echo ""
            echo "Examples:"
            echo "  $0                    # debug build, install to /Applications"
            echo "  $0 --release          # release build, install to /Applications"
            echo "  $0 --release --daemon # release build + serve-on-login LaunchAgent"
            exit 0
            ;;
        *)
            echo "${RED}Unknown option: $1${NORMAL}"
            echo "Use --help for usage."
            exit 1
            ;;
    esac
    shift
done

APP_INSTALL_DIR="/Applications"
BIN_INSTALL_DIR="$HOME/.local/bin"
APP_DEST="$APP_INSTALL_DIR/CuaDriver.app"
BIN_LINK="$BIN_INSTALL_DIR/cua-driver"

# Conditional sudo — matches install.sh. /Applications is usually
# group-writable by the admin group, so most users won't be prompted.
SUDO_APP=""
if [ ! -w "$APP_INSTALL_DIR" ]; then
    SUDO_APP="sudo"
fi

echo "${BOLD}${BLUE}cua-driver local installer${NORMAL}"
echo "Source:    ${BOLD}$CUA_DRIVER_DIR${NORMAL}"
echo "Config:    ${BOLD}$BUILD_CONFIG${NORMAL}"
echo "App path:  ${BOLD}$APP_DEST${NORMAL}"
echo "CLI path:  ${BOLD}$BIN_LINK${NORMAL}"
if [ "$INSTALL_DAEMON" = true ]; then
    echo "Daemon:    ${BOLD}LaunchAgent (cua-driver serve)${NORMAL}"
fi
echo ""

# --- Prerequisites ------------------------------------------------------

if ! command -v swift >/dev/null 2>&1; then
    echo "${RED}Error: swift not found on PATH.${NORMAL}"
    echo "Install Xcode Command Line Tools: xcode-select --install"
    exit 1
fi

# --- Build --------------------------------------------------------------

echo "${BOLD}Building cua-driver ($BUILD_CONFIG)...${NORMAL}"
cd "$CUA_DRIVER_DIR"
"$CUA_DRIVER_DIR/scripts/build-app.sh" "$BUILD_CONFIG"
echo ""

BUILD_APP="$CUA_DRIVER_DIR/.build/CuaDriver.app"
if [ ! -d "$BUILD_APP" ]; then
    echo "${RED}Error: build-app.sh did not produce $BUILD_APP${NORMAL}"
    exit 1
fi

# --- Remove stale dev-install paths -------------------------------------
#
# Older revisions of this script (and ad-hoc `install-cli.sh`, since
# removed) installed to `~/Applications/CuaDriver.app`. That leaks a
# second bundle that LaunchServices keys off
# `CFBundleIdentifier=com.trycua.driver`, which can silently re-route
# `cua-driver serve` to the stale `~/Applications` copy.
# Proactively remove them here so there is exactly one registered
# CuaDriver.app on the machine after every install.
STALE_APP="$HOME/Applications/CuaDriver.app"
for stale in "$STALE_APP"; do
    if [ -e "$stale" ] || [ -L "$stale" ]; then
        echo "Removing stale dev-install leftover: $stale"
        rm -rf "$stale"
    fi
done

# --- Install .app bundle ------------------------------------------------

echo "${BOLD}Installing CuaDriver.app to $APP_INSTALL_DIR...${NORMAL}"
$SUDO_APP mkdir -p "$APP_INSTALL_DIR"
if [ -e "$APP_DEST" ]; then
    $SUDO_APP rm -rf "$APP_DEST"
fi
# `ditto` preserves code signatures + extended attributes — `cp -R`
# strips Gatekeeper metadata on some macOS versions.
$SUDO_APP ditto "$BUILD_APP" "$APP_DEST"
echo "${GREEN}Installed $APP_DEST${NORMAL}"

# --- Install CLI symlink ------------------------------------------------

echo ""
echo "${BOLD}Linking cua-driver CLI into $BIN_INSTALL_DIR...${NORMAL}"
mkdir -p "$BIN_INSTALL_DIR"
if [ ! -w "$BIN_INSTALL_DIR" ]; then
    echo "${RED}Error: $BIN_INSTALL_DIR is not writable.${NORMAL}"
    echo "Pick a user-writable bin directory or fix ownership before rerunning."
    exit 1
fi
ln -sf "$APP_DEST/Contents/MacOS/cua-driver" "$BIN_LINK"
echo "${GREEN}Linked $BIN_LINK → $APP_DEST/Contents/MacOS/cua-driver${NORMAL}"

# --- Daemon (optional) --------------------------------------------------

if [ "$INSTALL_DAEMON" = true ]; then
    echo ""
    echo "${BOLD}Installing LaunchAgent (cua-driver serve)...${NORMAL}"
    SERVICE_NAME="com.trycua.cua_driver_daemon"
    PLIST_PATH="$HOME/Library/LaunchAgents/$SERVICE_NAME.plist"

    mkdir -p "$HOME/Library/LaunchAgents"
    if [ -f "$PLIST_PATH" ]; then
        launchctl unload "$PLIST_PATH" 2>/dev/null || true
    fi

    cat > "$PLIST_PATH" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$SERVICE_NAME</string>
    <key>ProgramArguments</key>
    <array>
        <string>$BIN_LINK</string>
        <string>serve</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/tmp/cua_driver_daemon.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/cua_driver_daemon.error.log</string>
    <key>ProcessType</key>
    <string>Interactive</string>
</dict>
</plist>
EOF
    chmod 644 "$PLIST_PATH"
    launchctl load "$PLIST_PATH"
    echo "${GREEN}LaunchAgent loaded: $PLIST_PATH${NORMAL}"
    echo "Logs: /tmp/cua_driver_daemon.log + /tmp/cua_driver_daemon.error.log"
else
    # Tear down any stale LaunchAgent from a prior run so two daemons
    # don't race on the default socket.
    SERVICE_NAME="com.trycua.cua_driver_daemon"
    PLIST_PATH="$HOME/Library/LaunchAgents/$SERVICE_NAME.plist"
    if [ -f "$PLIST_PATH" ]; then
        echo ""
        echo "Removing stale LaunchAgent (use --daemon to reinstall)..."
        launchctl unload "$PLIST_PATH" 2>/dev/null || true
        rm "$PLIST_PATH"
    fi
fi

# --- Summary ------------------------------------------------------------

cat <<EOF

${GREEN}${BOLD}cua-driver ($BUILD_CONFIG) installed.${NORMAL}

Next steps:
  1. First run: open $APP_DEST to grant TCC permissions via the
     setup window (Accessibility + Screen Recording).
  2. Verify the CLI:  $BIN_LINK --version
  3. Wire into an MCP client:
     $BIN_LINK mcp-config | pbcopy

Uninstall:  $CUA_DRIVER_DIR/scripts/uninstall.sh

${YELLOW}Note: this is a local build. Codesigning uses Developer ID when
available on the machine, ad-hoc otherwise — not notarized, so
Gatekeeper will prompt on first launch. Re-running install-local.sh
overwrites the previous build at $APP_DEST; TCC will re-prompt for
Accessibility + Screen Recording if the cdhash changed (typical for
ad-hoc-signed rebuilds).${NORMAL}
EOF

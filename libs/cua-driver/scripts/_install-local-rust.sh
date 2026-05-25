#!/usr/bin/env bash
#
# cua-driver-rs local/debug installer (macOS + Linux). Builds from the
# current source tree and drops the resulting cua-driver binary into the
# same install layout that scripts/install.sh produces — so a local build
# and a release install can coexist + the `current` symlink can flip
# between them.
#
# Private helper — invoked by install-local.sh (the multi-backend
# dispatcher) when the user picks --backend=rust / --experimental-rust
# or runs on a non-macOS host. Do not invoke directly; flag parity with
# the dispatcher's argv shape is maintained from there.
#
# Mirrors _install-local-swift.sh (the sibling Swift helper) in shape:
#   --release    build the release configuration (default: debug)
#   --autostart  register an auto-start daemon (macOS: LaunchAgent;
#                Linux: systemd user unit). Default off; the post-install
#                message prints the registration command for the platform.
#
# Not for end-users — scripts/install.sh fetches a built release from
# GitHub. This script is for the developer loop (rapid edit/build/test
# on a Linux or macOS host).
#
# Linux layout produced (matches install.sh):
#
#   ${CUA_DRIVER_RS_HOME:-$HOME/.cua-driver-rs}/packages/
#       releases/<version>-local-<config>-<target>/cua-driver
#       current/cua-driver  -> ../releases/<active>/cua-driver
#   ${CUA_DRIVER_RS_INSTALL_DIR:-$HOME/.local/bin}/cua-driver
#       -> ../current/cua-driver
#
# macOS layout produced:
#   /Applications/CuaDriver.app/Contents/MacOS/cua-driver  (bundle replaced wholesale)
#   $HOME/.local/bin/cua-driver -> .../CuaDriver.app/Contents/MacOS/cua-driver
#
# The version string carries `-local-debug` / `-local-release` so it
# never collides with a real release dir and is trivial to GC.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# Rust workspace root: scripts/ is the cross-cutting installer dir at
# libs/cua-driver/scripts/; the Cargo workspace lives one level deeper
# under libs/cua-driver/rust/.
REPO_ROOT="$(cd "$SCRIPT_DIR/../rust" && pwd)"

# --- Load shared daemon-cleanup helpers ---------------------------------
#
# Sibling _install-common.sh defines stop_cua_driver_daemons +
# show_cua_driver_daemon_survivors, mirroring CuaDriverInstall.psm1.
# This is a dev-only path always invoked from a checked-out tree (the
# `install-local.sh` dispatcher only runs from a clone), so the on-disk
# load is the only branch we need — no curl fallback like _install-rust.sh.
# Define no-op stubs if the file is missing so call sites stay
# unconditional and a stale clone doesn't fail the install.
if [ -f "$SCRIPT_DIR/_install-common.sh" ]; then
    # shellcheck source=_install-common.sh
    . "$SCRIPT_DIR/_install-common.sh"
else
    echo "warning: $SCRIPT_DIR/_install-common.sh missing; daemon kill skipped" >&2
    stop_cua_driver_daemons() { :; }
    show_cua_driver_daemon_survivors() { :; }
fi

BOLD=$(tput bold 2>/dev/null || true)
NORMAL=$(tput sgr0 2>/dev/null || true)
RED=$(tput setaf 1 2>/dev/null || true)
GREEN=$(tput setaf 2 2>/dev/null || true)
BLUE=$(tput setaf 4 2>/dev/null || true)

if [ "$(id -u)" -eq 0 ] || [ -n "${SUDO_USER:-}" ]; then
    echo "${RED}Error: do not run this script with sudo or as root.${NORMAL}"
    echo "It prompts for sudo on the specific operations that need it."
    exit 1
fi

# --- Parse arguments ----------------------------------------------------

BUILD_CONFIG="debug"
INSTALL_AUTOSTART=false

while [ "$#" -gt 0 ]; do
    case "$1" in
        --release)
            BUILD_CONFIG="release"
            ;;
        --autostart)
            INSTALL_AUTOSTART=true
            ;;
        --help|-h)
            echo "${BOLD}${BLUE}cua-driver-rs local installer${NORMAL}"
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --release     Build the release configuration (default: debug)."
            echo "  --autostart   Also register a logon-time daemon:"
            echo "                  macOS: LaunchAgent under ~/Library/LaunchAgents"
            echo "                  Linux: systemd --user unit"
            echo "  --help        Show this help."
            echo ""
            echo "Examples:"
            echo "  $0                       # debug build, install junction layout"
            echo "  $0 --release             # release build"
            echo "  $0 --release --autostart # release + daemon at logon"
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

OS="$(uname -s)"
ARCH="$(uname -m)"
case "$OS" in
    Darwin) TARGET_TRIPLE="${ARCH}-apple-darwin" ;;
    Linux)  TARGET_TRIPLE="${ARCH}-unknown-linux-gnu" ;;
    *)      echo "${RED}Unsupported OS: $OS${NORMAL}"; exit 1 ;;
esac

HOME_DIR="${CUA_DRIVER_RS_HOME:-$HOME/.cua-driver-rs}"
BIN_DIR="${CUA_DRIVER_RS_INSTALL_DIR:-${CUA_DRIVER_RS_BIN_DIR:-$HOME/.local/bin}}"
RELEASES_DIR="$HOME_DIR/packages/releases"
CURRENT_LINK="$HOME_DIR/packages/current"

VERSION_TAG="0.0.0-local-$BUILD_CONFIG"
VERSIONED_DIR="$RELEASES_DIR/$VERSION_TAG-$TARGET_TRIPLE"

echo "${BOLD}${BLUE}cua-driver-rs local installer${NORMAL}"
echo "  source:  ${BOLD}$REPO_ROOT${NORMAL}"
echo "  config:  ${BOLD}$BUILD_CONFIG${NORMAL}"
echo "  target:  ${BOLD}$TARGET_TRIPLE${NORMAL}"
echo "  bin:     ${BOLD}$BIN_DIR/cua-driver${NORMAL}"
echo "  current: ${BOLD}$CURRENT_LINK${NORMAL}"
echo ""

# --- Prerequisites ------------------------------------------------------

if ! command -v cargo >/dev/null 2>&1; then
    echo "${RED}Error: cargo not found on PATH.${NORMAL}"
    echo "Install Rust via rustup: https://rustup.rs/"
    exit 1
fi

# --- Build --------------------------------------------------------------

echo "${BOLD}Building cua-driver ($BUILD_CONFIG)...${NORMAL}"
cd "$REPO_ROOT"
if [ "$BUILD_CONFIG" = "release" ]; then
    cargo build --release -p cua-driver
else
    cargo build -p cua-driver
fi

BUILT_BINARY="$REPO_ROOT/target/$BUILD_CONFIG/cua-driver"
if [ ! -x "$BUILT_BINARY" ]; then
    echo "${RED}Error: build produced no binary at $BUILT_BINARY${NORMAL}"
    exit 1
fi
echo ""

# --- Stage into versioned release dir + repoint `current` --------------

echo "${BOLD}Staging into $VERSIONED_DIR${NORMAL}"
mkdir -p "$VERSIONED_DIR"
cp "$BUILT_BINARY" "$VERSIONED_DIR/cua-driver"
chmod +x "$VERSIONED_DIR/cua-driver"

# Skill pack — stage from the repo so the `current` symlink below
# transparently exposes it to agents. Mirrors what install.sh does
# from a release tarball.
SOURCE_SKILLS="$REPO_ROOT/Skills/cua-driver-rs"
if [ -d "$SOURCE_SKILLS" ]; then
    STAGED_SKILLS="$VERSIONED_DIR/Skills/cua-driver-rs"
    rm -rf "$STAGED_SKILLS"
    mkdir -p "$(dirname "$STAGED_SKILLS")"
    cp -R "$SOURCE_SKILLS" "$STAGED_SKILLS"
    echo "${GREEN}staged skill pack at $STAGED_SKILLS${NORMAL}"
fi

# Atomic-ish swap of the `current` symlink.
mkdir -p "$HOME_DIR/packages"
TMP_LINK="$CURRENT_LINK.new"
rm -f "$TMP_LINK"
ln -s "$VERSIONED_DIR" "$TMP_LINK"
mv -Tf "$TMP_LINK" "$CURRENT_LINK" 2>/dev/null || mv -f "$TMP_LINK" "$CURRENT_LINK"
echo "${GREEN}current -> $VERSIONED_DIR${NORMAL}"
echo ""

# --- Visible-bin symlink ------------------------------------------------

mkdir -p "$BIN_DIR"
ln -sf "$CURRENT_LINK/cua-driver" "$BIN_DIR/cua-driver"
echo "${GREEN}$BIN_DIR/cua-driver -> $CURRENT_LINK/cua-driver${NORMAL}"
echo ""

INSTALLED_BIN="$BIN_DIR/cua-driver"

# --- Stop any pre-swap cua-driver daemons ------------------------------
#
# Mirror of install-local.ps1's daemon kill — the new binary is now
# under packages/current/, but any LaunchAgent / systemd user unit /
# manual `serve` shell is still running off the OLD binary. Stop them
# so the next invocation picks up this build. Best-effort, never
# fails the install. Survivors (rare on Unix — `pkill` reaches all
# user-owned procs without elevation) get a yellow hint.
stop_cua_driver_daemons
show_cua_driver_daemon_survivors

# Agent skill pack symlinks: NOT auto-created. Run
# `cua-driver skills install --local` to symlink agent dirs to the
# staged copy at $VERSIONED_DIR/Skills/cua-driver-rs above.
echo ""

# --- Autostart (optional) ----------------------------------------------

if [ "$INSTALL_AUTOSTART" = true ]; then
    if [ "$OS" = "Darwin" ]; then
        PLIST_PATH="$HOME/Library/LaunchAgents/com.trycua.cua-driver-rs.plist"
        echo "${BOLD}Writing LaunchAgent → $PLIST_PATH${NORMAL}"
        mkdir -p "$(dirname "$PLIST_PATH")"
        cat >"$PLIST_PATH" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.trycua.cua-driver-rs</string>
  <key>ProgramArguments</key>
  <array>
    <string>$INSTALLED_BIN</string>
    <string>serve</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>$HOME_DIR/serve.out.log</string>
  <key>StandardErrorPath</key><string>$HOME_DIR/serve.err.log</string>
</dict>
</plist>
EOF
        launchctl unload "$PLIST_PATH" 2>/dev/null || true
        launchctl load "$PLIST_PATH"
        echo "${GREEN}Loaded.${NORMAL} Manage with launchctl load / unload \"$PLIST_PATH\"."
    elif [ "$OS" = "Linux" ]; then
        UNIT_PATH="$HOME/.config/systemd/user/cua-driver-rs.service"
        echo "${BOLD}Writing systemd user unit → $UNIT_PATH${NORMAL}"
        mkdir -p "$(dirname "$UNIT_PATH")"
        cat >"$UNIT_PATH" <<EOF
[Unit]
Description=cua-driver-rs serve daemon
After=graphical-session.target

[Service]
ExecStart=$INSTALLED_BIN serve
Restart=on-failure
RestartSec=2

[Install]
WantedBy=default.target
EOF
        systemctl --user daemon-reload
        systemctl --user enable --now cua-driver-rs.service
        echo "${GREEN}Enabled.${NORMAL} Manage with systemctl --user {start|stop|status} cua-driver-rs."
    fi
    echo ""
fi

# --- Done ---------------------------------------------------------------

echo "${BOLD}${GREEN}Installed.${NORMAL}"
echo "  ${BOLD}$INSTALLED_BIN${NORMAL}"
echo ""

# Unified post-install hints come from a single shared text file so the
# 4 Rust installers (this script + install-local.ps1 + _install-rust.sh +
# install.ps1) never drift. The .txt holds the OS-agnostic bulk
# (Try-it / skill pack / MCP setup / docs link) with {{BINARY}}
# placeholders; OS-specific bits stay inline below.
HINTS_TXT="$SCRIPT_DIR/post-install-hints.txt"
if [ -f "$HINTS_TXT" ]; then
    sed "s|{{BINARY}}|$INSTALLED_BIN|g" "$HINTS_TXT"
else
    # Repo layout changed or running from an unexpected location — fall
    # back to one-line essentials so users still know what to do next.
    echo "Next steps: $INSTALLED_BIN --version  |  $INSTALLED_BIN mcp-config  |  $INSTALLED_BIN skills install"
    echo "Docs: https://github.com/trycua/cua/tree/main/libs/cua-driver/rust"
fi

# OS-specific autostart hint (kept inline; per-shell natural location).
if [ "$INSTALL_AUTOSTART" != true ]; then
    echo ""
    if [ "$OS" = "Darwin" ]; then
        echo "Auto-start (optional): re-run with --autostart to register a LaunchAgent."
    else
        echo "Auto-start (optional): re-run with --autostart to register a systemd user unit."
    fi
    echo ""
fi

#!/bin/bash
#
# setup-cua-computer.sh
# Makes a Lume macOS VM image cua-computer compatible
#
# This script runs ON the VM itself (not the host).
# It installs cua-computer-server, cua-agent, Playwright, and configures
# the VM for headless CUA operation (auto-login, disable sleep, LaunchAgent).
#
# Usage:
#   lume ssh <vm-name> "curl -fsSL <url> | bash"
#   lume ssh <vm-name> < setup-cua-computer.sh
#   # Or copy and run directly inside the VM:
#   ./setup-cua-computer.sh [--yes] [--port PORT]
#
# Prerequisites:
#   - macOS VM created via lume (with SSH enabled, user lume/lume)
#

set -e

# ============================================
# Configuration
# ============================================

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

AUTO_YES=false
CUA_SERVER_PORT=8443
VM_PASSWORD="${VM_PASSWORD:-lume}"
CUA_DIR="$HOME/.cua-server"

# Parse arguments
while [ "$#" -gt 0 ]; do
    case "$1" in
        --yes|-y)
            AUTO_YES=true
            ;;
        --port)
            CUA_SERVER_PORT="$2"
            shift
            ;;
        --help)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --yes, -y       Non-interactive mode (accept all prompts)"
            echo "  --port PORT     CUA computer server port (default: 8443)"
            echo "  --help          Show this help message"
            exit 0
            ;;
        *)
            echo -e "${RED}Unknown option: $1${NC}"
            exit 1
            ;;
    esac
    shift
done

CURRENT_USER="$(whoami)"
USER_HOME="$HOME"

# ============================================
# Helpers
# ============================================

print_phase() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}  $1${NC}"
    echo -e "${BLUE}========================================${NC}"
    echo ""
}

confirm() {
    local prompt="$1"
    if [ "$AUTO_YES" = true ]; then
        echo -e "${YELLOW}${prompt} [Y/n] ${NC}y (auto)"
        return 0
    fi
    read -r -p "$(echo -e "${YELLOW}${prompt} [Y/n] ${NC}")" response
    case "$response" in
        [nN][oO]|[nN]) return 1 ;;
        *) return 0 ;;
    esac
}

ensure_brew() {
    if [ -f /opt/homebrew/bin/brew ]; then
        eval "$(/opt/homebrew/bin/brew shellenv)"
    fi
}

# Create an askpass helper for non-interactive sudo
setup_sudo() {
    if [ "$AUTO_YES" = true ]; then
        ASKPASS_SCRIPT=$(mktemp)
        printf '#!/bin/bash\necho "%s"\n' "$VM_PASSWORD" > "$ASKPASS_SCRIPT"
        chmod +x "$ASKPASS_SCRIPT"
        export SUDO_ASKPASS="$ASKPASS_SCRIPT"
        sudo -A -v 2>/dev/null || true
    fi
}

cleanup_sudo() {
    if [ -n "${ASKPASS_SCRIPT:-}" ] && [ -f "$ASKPASS_SCRIPT" ]; then
        rm -f "$ASKPASS_SCRIPT"
    fi
}
trap cleanup_sudo EXIT

# ============================================
# Phase 1: Prerequisites (Homebrew + Python)
# ============================================

install_homebrew() {
    print_phase "Phase 1: Homebrew"

    if command -v brew &>/dev/null; then
        echo -e "${GREEN}Homebrew already installed$(brew --version | head -1 | sed 's/Homebrew //' | sed 's/^/ (v/')${NC})"
        ensure_brew
        return 0
    fi

    if ! confirm "Install Homebrew?"; then
        echo -e "${YELLOW}Skipping Homebrew. Later steps will fail.${NC}"
        return 1
    fi

    setup_sudo

    if [ "$AUTO_YES" = true ]; then
        NONINTERACTIVE=1 SUDO_ASKPASS="$ASKPASS_SCRIPT" /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
    else
        echo -e "${YELLOW}Homebrew requires sudo access. Enter your password if prompted.${NC}"
        /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
    fi

    # Add brew to shell profile
    if ! grep -q 'brew shellenv' "$HOME/.zprofile" 2>/dev/null; then
        echo 'eval "$(/opt/homebrew/bin/brew shellenv)"' >> "$HOME/.zprofile"
    fi

    ensure_brew
    echo -e "${GREEN}Homebrew installed${NC}"
}

install_python() {
    print_phase "Phase 2: Python 3.13"

    ensure_brew

    if brew list python@3.13 &>/dev/null; then
        echo -e "${GREEN}Python 3.13 already installed${NC}"
    else
        if ! confirm "Install Python 3.13 via Homebrew?"; then
            echo -e "${YELLOW}Skipping Python 3.13.${NC}"
            return 1
        fi
        brew install python@3.13
        echo -e "${GREEN}Python 3.13 installed${NC}"
    fi

    export PATH="/opt/homebrew/opt/python@3.13/bin:$PATH"
}

install_system_deps() {
    print_phase "Phase 3: System Dependencies"

    ensure_brew

    local deps_needed=()
    for pkg in ffmpeg; do
        if ! brew list "$pkg" &>/dev/null; then
            deps_needed+=("$pkg")
        fi
    done

    if [ ${#deps_needed[@]} -eq 0 ]; then
        echo -e "${GREEN}All system dependencies already installed${NC}"
        return 0
    fi

    if confirm "Install system dependencies (${deps_needed[*]})?"; then
        brew install "${deps_needed[@]}"
        echo -e "${GREEN}System dependencies installed${NC}"
    fi
}

# ============================================
# Phase 4: CUA Server + Agent
# ============================================

setup_cua_server() {
    print_phase "Phase 4: CUA Computer Server & Agent"

    ensure_brew
    export PATH="/opt/homebrew/opt/python@3.13/bin:$PATH"

    if ! confirm "Install cua-computer-server and cua-agent?"; then
        echo -e "${YELLOW}Skipping CUA server setup.${NC}"
        return 1
    fi

    mkdir -p "$CUA_DIR"

    # Create venv if needed
    if [ ! -d "$CUA_DIR/venv" ]; then
        echo "Creating Python 3.13 virtual environment..."
        /opt/homebrew/opt/python@3.13/bin/python3.13 -m venv "$CUA_DIR/venv"
    fi

    echo "Installing cua-computer-server..."
    source "$CUA_DIR/venv/bin/activate"
    pip install --upgrade pip
    pip install cua-computer-server

    echo "Installing cua-agent..."
    pip install 'cua-agent[all]'

    # Playwright + Firefox (non-blocking)
    echo "Installing Playwright and Firefox browser..."
    pip install playwright && playwright install firefox || \
        echo -e "${YELLOW}Playwright install failed (non-blocking, BrowserTool may not work)${NC}"

    deactivate

    echo -e "${GREEN}CUA server and agent installed${NC}"
}

# ============================================
# Phase 5: Startup Script + LaunchAgent
# ============================================

create_startup_script() {
    print_phase "Phase 5: Startup Script & LaunchAgent"

    if ! confirm "Create CUA server startup script and LaunchAgent?"; then
        echo -e "${YELLOW}Skipping LaunchAgent setup.${NC}"
        return 1
    fi

    local STARTUP_SCRIPT="$CUA_DIR/start_server.sh"

    cat > "$STARTUP_SCRIPT" <<'SCRIPT_EOF'
#!/bin/bash

LOG_FILE="$HOME/.cua-server/server.log"
echo "=== CUA Server startup at $(date) ===" >> "$LOG_FILE"

# Wait for window server
sleep 10

cd "$HOME/.cua-server"
source venv/bin/activate

export DISPLAY=:0

# Update packages
echo "Updating cua-agent..." >> "$LOG_FILE"
pip install --upgrade --no-input "cua-agent[all]" >> "$LOG_FILE" 2>&1 || true

echo "Updating cua-computer-server..." >> "$LOG_FILE"
pip install --upgrade --no-input cua-computer-server >> "$LOG_FILE" 2>&1 || true

# Ensure Playwright Firefox
echo "Ensuring Playwright Firefox..." >> "$LOG_FILE"
pip install --upgrade --no-input playwright >> "$LOG_FILE" 2>&1 || true
playwright install firefox >> "$LOG_FILE" 2>&1 || true

# Start server
echo "Starting CUA computer server on port __PORT__..." >> "$LOG_FILE"
python -m computer_server --port __PORT__ >> "$LOG_FILE" 2>&1
SCRIPT_EOF

    # Stamp the port
    sed -i '' "s/__PORT__/$CUA_SERVER_PORT/g" "$STARTUP_SCRIPT"
    chmod +x "$STARTUP_SCRIPT"

    # Create LaunchAgent
    local SERVICE_NAME="com.trycua.computer_server"
    local PLIST_PATH="$USER_HOME/Library/LaunchAgents/$SERVICE_NAME.plist"

    mkdir -p "$USER_HOME/Library/LaunchAgents"

    # Unload existing
    launchctl unload "$PLIST_PATH" 2>/dev/null || true

    cat > "$PLIST_PATH" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$SERVICE_NAME</string>

    <key>WorkingDirectory</key>
    <string>$CUA_DIR</string>

    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>/opt/homebrew/bin:/opt/homebrew/opt/python@3.13/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
        <key>HOME</key>
        <string>$USER_HOME</string>
        <key>DISPLAY</key>
        <string>:0</string>
    </dict>

    <key>ProgramArguments</key>
    <array>
        <string>/bin/bash</string>
        <string>$CUA_DIR/start_server.sh</string>
    </array>

    <key>RunAtLoad</key>
    <true/>

    <key>KeepAlive</key>
    <true/>

    <key>StandardOutPath</key>
    <string>/tmp/computer_server.log</string>

    <key>StandardErrorPath</key>
    <string>/tmp/computer_server.error.log</string>

    <key>ProcessType</key>
    <string>Interactive</string>

    <key>SessionType</key>
    <string>Aqua</string>
</dict>
</plist>
EOF

    chmod 644 "$PLIST_PATH"
    touch /tmp/computer_server.log /tmp/computer_server.error.log

    echo -e "${GREEN}Startup script and LaunchAgent created${NC}"
    echo "LaunchAgent will start CUA server on port $CUA_SERVER_PORT at login"
}

# ============================================
# Phase 6: System Configuration
# ============================================

configure_system() {
    print_phase "Phase 6: System Configuration"

    if ! confirm "Disable screen lock, screensaver, and sleep?"; then
        echo -e "${YELLOW}Skipping system configuration.${NC}"
        return 0
    fi

    setup_sudo

    # Disable screensaver
    defaults -currentHost write com.apple.screensaver idleTime 0
    defaults write com.apple.screensaver askForPassword -int 0
    defaults write com.apple.screensaver askForPasswordDelay -int 0

    # Disable sleep
    if [ "$AUTO_YES" = true ]; then
        sudo -A pmset -a displaysleep 0 sleep 0 disksleep 0
    else
        sudo pmset -a displaysleep 0 sleep 0 disksleep 0
    fi

    # Enable full keyboard navigation (needed for accessibility automation)
    defaults write NSGlobalDomain AppleKeyboardUIMode -int 3

    echo -e "${GREEN}System configured (no sleep, no screensaver, full keyboard nav)${NC}"
}

# ============================================
# Phase 7: Verification
# ============================================

verify() {
    print_phase "Verification"

    echo "System:"
    echo "  User:    $CURRENT_USER"
    sw_vers | sed 's/^/  /'

    echo ""
    echo "Installed:"

    # Homebrew
    if command -v brew &>/dev/null; then
        echo -e "  ${GREEN}✓${NC} Homebrew $(brew --version | head -1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+')"
    else
        echo -e "  ${RED}✗${NC} Homebrew"
    fi

    # Python
    ensure_brew
    if brew list python@3.13 &>/dev/null; then
        echo -e "  ${GREEN}✓${NC} Python 3.13"
    else
        echo -e "  ${RED}✗${NC} Python 3.13"
    fi

    # CUA server
    if [ -f "$CUA_DIR/venv/bin/python" ]; then
        local server_ver
        server_ver=$("$CUA_DIR/venv/bin/pip" show cua-computer-server 2>/dev/null | grep Version | awk '{print $2}')
        echo -e "  ${GREEN}✓${NC} cua-computer-server ${server_ver:-unknown}"
    else
        echo -e "  ${RED}✗${NC} cua-computer-server"
    fi

    # Agent
    if [ -f "$CUA_DIR/venv/bin/python" ]; then
        local agent_ver
        agent_ver=$("$CUA_DIR/venv/bin/pip" show cua-agent 2>/dev/null | grep Version | awk '{print $2}')
        echo -e "  ${GREEN}✓${NC} cua-agent ${agent_ver:-unknown}"
    else
        echo -e "  ${RED}✗${NC} cua-agent"
    fi

    # Playwright
    if "$CUA_DIR/venv/bin/python" -c "import playwright" 2>/dev/null; then
        echo -e "  ${GREEN}✓${NC} Playwright + Firefox"
    else
        echo -e "  ${YELLOW}~${NC} Playwright (may not be installed)"
    fi

    # LaunchAgent
    local PLIST="$USER_HOME/Library/LaunchAgents/com.trycua.computer_server.plist"
    if [ -f "$PLIST" ]; then
        echo -e "  ${GREEN}✓${NC} LaunchAgent (port $CUA_SERVER_PORT)"
    else
        echo -e "  ${RED}✗${NC} LaunchAgent"
    fi

    echo ""
    echo -e "${GREEN}========================================${NC}"
    echo -e "${GREEN}  Setup Complete!${NC}"
    echo -e "${GREEN}========================================${NC}"
    echo ""
    echo "CUA computer server will start on port $CUA_SERVER_PORT at next login."
    echo ""
    echo "To start it now (from the host):"
    echo "  lume ssh <vm-name> 'launchctl load ~/Library/LaunchAgents/com.trycua.computer_server.plist'"
    echo ""
    echo "To test from the host:"
    echo "  curl http://<vm-ip>:$CUA_SERVER_PORT/health"
    echo ""
}

# ============================================
# Main
# ============================================

echo -e "${BLUE}"
echo "  ⠀⣀⣀⡀⠀⠀⠀⠀⢀⣀⣀⣀⡀⠘⠋⢉⠙⣷⠀⠀ ⠀"
echo " ⠀⠀⢀⣴⣿⡿⠋⣉⠁⣠⣾⣿⣿⣿⣿⡿⠿⣦⡈⠀⣿⡇⠃⠀"
echo " ⠀⠀⠀⣽⣿⣧⠀⠃⢰⣿⣿⡏⠙⣿⠿⢧⣀⣼⣷⠀⡿⠃⠀⠀"
echo " ⠀⠀⠀⠉⣿⣿⣦⠀⢿⣿⣿⣷⣾⡏⠀⠀⢹⣿⣿⠀⠀⠀⠀⠀⠀"
echo -e " ⠀⠀⠀⠀⠀⠉⠛⠁⠈⠿⣿⣿⣿⣷⣄⣠⡼⠟⠁${NC}  ${BLUE}CUA Computer Setup${NC}"
echo -e "${BLUE}           Make a Lume VM cua-computer compatible${NC}"
echo ""

install_homebrew
install_python
install_system_deps
setup_cua_server
create_startup_script
configure_system
verify

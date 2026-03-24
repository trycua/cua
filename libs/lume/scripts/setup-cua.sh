#!/bin/bash
#
# setup-cua.sh
# Makes a Lume macOS VM fully cua-compatible
#
# This script runs ON the VM itself (not the host).
# It installs cua-computer-server, cua-agent, Playwright, noVNC (web-based VNC
# proxying to lume's built-in VNC on the host), supervisor, and configures the
# VM for headless CUA operation (auto-login, disable sleep, LaunchAgent).
#
# In Lume VMs, the display is provided by the Virtualization Framework and
# exposed through lume's built-in VNC server on the host. macOS Screen Sharing
# inside the VM has no framebuffer to capture. Therefore, noVNC/websockify
# inside the VM proxies to lume's VNC on the host (via the gateway IP).
#
# Usage:
#   # Copy the script into the VM first, then run with a generous timeout:
#   lume ssh <vm-name> "cat > /tmp/setup.sh && chmod +x /tmp/setup.sh" < setup-cua.sh
#   lume ssh <vm-name> --timeout 600 "bash /tmp/setup.sh --yes"
#
#   # Or copy and run directly inside the VM:
#   ./setup-cua.sh [--yes] [--port PORT] [--host-vnc-port PORT]
#
# VNC: Lume auto-generates a random VNC port and password for each VM and
# delivers them to ~/.vnc.env inside the VM via SSH. The startup script and
# websockify read this file automatically — no manual --vnc-port or
# --vnc-password flags are needed.
#
# Accessing noVNC remotely (from outside the host):
#   1. SSH tunnel: ssh -L 6080:<vm-ip>:6080 user@host
#   2. Open: http://localhost:6080/vnc.html?host=localhost&port=6080
#   The ?host=localhost&port=6080 params ensure the websocket connects through
#   the tunnel instead of trying to reach the VM IP directly.
#
# Note: The default lume ssh timeout is 60s, which is too short for this script
# (Homebrew + Xcode CLT + pip installs can take 5-10 minutes). Use --timeout 600
# or --timeout 0 (unlimited) when running via lume ssh.
#
# Prerequisites:
#   - macOS VM created via lume (with SSH enabled, user lume/lume)
#
# Host troubleshooting:
#   If VMs fail to start with "Unable to access security information", the host's
#   login keychain is likely locked (common after host reboot/crash on macOS Sequoia+).
#   Fix: security unlock-keychain -p '<host-password>' ~/Library/Keychains/login.keychain-db
#   Prevention: enable auto-login on the host so the keychain unlocks at boot.
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
NOVNC_PORT=6080
HOST_VNC_PORT="${HOST_VNC_PORT:-}"
VNC_PASSWORD="${VNC_PASSWORD:-}"

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
        --host-vnc-port)
            echo -e "${YELLOW}Note: --host-vnc-port is deprecated. Lume auto-generates VNC ports.${NC}"
            HOST_VNC_PORT="$2"
            shift
            ;;
        --vnc-password)
            echo -e "${YELLOW}Note: --vnc-password is deprecated. Lume auto-generates VNC passwords.${NC}"
            VNC_PASSWORD="$2"
            shift
            ;;
        --help)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --yes, -y                  Non-interactive mode (accept all prompts)"
            echo "  --port PORT                CUA computer server port (default: 8443)"
            echo "  --host-vnc-port PORT       (deprecated) Lume auto-generates VNC ports"
            echo "  --vnc-password PWD         (deprecated) Lume auto-generates VNC passwords"
            echo "  --help                     Show this help message"
            echo ""
            echo "Environment variables:"
            echo "  VM_PASSWORD                VM user password (default: lume)"
            echo "  HOST_VNC_PORT              (deprecated) Lume auto-generates VNC ports"
            echo "  VNC_PASSWORD               (deprecated) Lume auto-generates VNC passwords"
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

# Auto-detect host gateway IP
GATEWAY_IP=$(route get default 2>/dev/null | grep gateway | awk '{print $2}')
if [ -z "$GATEWAY_IP" ]; then
    GATEWAY_IP="192.168.64.1"
fi

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
    for pkg in ffmpeg supervisor wget; do
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
    pip install "cua-computer-server[vnc]"

    echo "Installing cua-agent..."
    pip install 'cua-agent[all]'

    # Playwright + Firefox (non-blocking)
    echo "Installing Playwright and Firefox browser..."
    pip install playwright && playwright install firefox || \
        echo -e "${YELLOW}Playwright install failed (non-blocking, BrowserTool may not work)${NC}"

    # Install websockify for noVNC and vncdotool for VNC backend
    echo "Installing websockify and vncdotool..."
    pip install websockify vncdotool

    deactivate

    echo -e "${GREEN}CUA server and agent installed${NC}"
}

# ============================================
# Phase 5: noVNC + Websockify
# ============================================

setup_novnc() {
    print_phase "Phase 5: noVNC (Web-based VNC)"

    if ! confirm "Install noVNC and websockify (web VNC on port $NOVNC_PORT)?"; then
        echo -e "${YELLOW}Skipping noVNC setup.${NC}"
        return 0
    fi

    ensure_brew

    local NOVNC_DIR="/usr/local/share/novnc"

    setup_sudo

    # Download noVNC
    echo "Installing noVNC..."
    cd /tmp
    rm -rf noVNC-master noVNC-master.zip
    wget -q https://github.com/trycua/noVNC/archive/refs/heads/master.zip -O noVNC-master.zip
    unzip -q noVNC-master.zip

    if [ "$AUTO_YES" = true ]; then
        sudo -A rm -rf "$NOVNC_DIR"
        sudo -A mkdir -p "$NOVNC_DIR"
        sudo -A mv noVNC-master/* "$NOVNC_DIR/"
        sudo -A ln -sf "$NOVNC_DIR/vnc.html" "$NOVNC_DIR/index.html"
        sudo -A chmod -R 755 "$NOVNC_DIR"
    else
        sudo rm -rf "$NOVNC_DIR"
        sudo mkdir -p "$NOVNC_DIR"
        sudo mv noVNC-master/* "$NOVNC_DIR/"
        sudo ln -sf "$NOVNC_DIR/vnc.html" "$NOVNC_DIR/index.html"
        sudo chmod -R 755 "$NOVNC_DIR"
    fi

    rm -rf noVNC-master noVNC-master.zip

    # Configure supervisor for websockify
    # Proxies to lume's built-in VNC on the host via the gateway IP.
    # The VNC port is read from ~/.vnc.env (written by lume via SSH).
    local SUPERVISOR_CONF_DIR="/opt/homebrew/etc/supervisor.d"
    local SUPERVISOR_LOG_DIR="/opt/homebrew/var/log/supervisor"
    local VENV_PYTHON="$CUA_DIR/venv/bin/python3"

    mkdir -p "$SUPERVISOR_CONF_DIR"
    mkdir -p "$SUPERVISOR_LOG_DIR"

    # Create a wrapper script that reads VNC config from ~/.vnc.env
    local WEBSOCKIFY_WRAPPER="$CUA_DIR/start_websockify.sh"
    cat > "$WEBSOCKIFY_WRAPPER" <<'WSEOF'
#!/bin/bash
# Read VNC port from ~/.vnc.env (written by lume via SSH after VM boot)
VNC_PORT=""
if [ -s "$HOME/.vnc.env" ]; then
    eval "$(cat "$HOME/.vnc.env")"
fi
if [ -z "$VNC_PORT" ]; then
    echo "No ~/.vnc.env found — websockify cannot start without VNC port" >&2
    exit 1
fi

GATEWAY_IP=$(route get default 2>/dev/null | grep gateway | awk '{print $2}')
if [ -z "$GATEWAY_IP" ]; then
    GATEWAY_IP="192.168.64.1"
fi

exec __VENV_PYTHON__ -m websockify --web __NOVNC_DIR__ __NOVNC_PORT__ "$GATEWAY_IP:$VNC_PORT"
WSEOF
    sed -i '' "s|__VENV_PYTHON__|$VENV_PYTHON|g" "$WEBSOCKIFY_WRAPPER"
    sed -i '' "s|__NOVNC_DIR__|$NOVNC_DIR|g" "$WEBSOCKIFY_WRAPPER"
    sed -i '' "s|__NOVNC_PORT__|$NOVNC_PORT|g" "$WEBSOCKIFY_WRAPPER"
    chmod +x "$WEBSOCKIFY_WRAPPER"

    cat > "$SUPERVISOR_CONF_DIR/websockify.ini" <<EOF
[program:websockify]
command=/bin/bash $WEBSOCKIFY_WRAPPER
directory=$NOVNC_DIR
autostart=true
autorestart=true
startsecs=5
startretries=3
stopwaitsecs=10
redirect_stderr=true
stdout_logfile=$SUPERVISOR_LOG_DIR/websockify.log
stdout_logfile_maxbytes=10MB
stdout_logfile_backups=5
environment=HOME="$USER_HOME",PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin",DISPLAY=":0"
EOF

    chmod 644 "$SUPERVISOR_CONF_DIR/websockify.ini"

    echo -e "${GREEN}noVNC installed${NC}"
    echo "  websockify will proxy port $NOVNC_PORT -> lume VNC (port from ~/.vnc.env)"
}

# ============================================
# Phase 6: Startup Script + LaunchAgent
# ============================================

create_startup_script() {
    print_phase "Phase 6: Startup Script & LaunchAgent"

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

# Auto-detect host gateway IP for VNC backend
GATEWAY_IP=$(route get default 2>/dev/null | grep gateway | awk '{print $2}')
if [ -z "$GATEWAY_IP" ]; then
    GATEWAY_IP="192.168.64.1"
fi

# Read VNC config — check multiple sources in priority order:
# 1. Local vnc.env (written by host via lume ssh, or cached from VirtioFS)
# 2. VirtioFS lume-config mount (may be blocked by macOS TCC in LaunchAgents)
LUME_VNC_PORT=""
LUME_VNC_PASSWORD=""
LOCAL_VNC_ENV="$HOME/.vnc.env"

# Source 1: local vnc.env (most reliable — not subject to TCC restrictions)
if [ -s "$LOCAL_VNC_ENV" ]; then
    eval "$(cat "$LOCAL_VNC_ENV")"
    LUME_VNC_PORT="${VNC_PORT:-}"
    LUME_VNC_PASSWORD="${VNC_PASSWORD:-}"
    echo "Read VNC config from local vnc.env: port=$LUME_VNC_PORT" >> "$LOG_FILE"
fi

# Source 2: VirtioFS lume-config (fallback — may need sudo due to TCC)
if [ -z "$LUME_VNC_PORT" ]; then
    LUME_CONFIG_MOUNT="/tmp/lume-config"
    LUME_CONFIG="$LUME_CONFIG_MOUNT/vnc.env"
    mkdir -p "$LUME_CONFIG_MOUNT" 2>/dev/null
    if mount | grep -q "lume-config"; then
        echo "lume-config already mounted" >> "$LOG_FILE"
    elif mount_virtiofs lume-config "$LUME_CONFIG_MOUNT" 2>/dev/null; then
        echo "Mounted lume-config VirtioFS share" >> "$LOG_FILE"
    else
        echo "lume-config VirtioFS not available" >> "$LOG_FILE"
    fi

    # Try reading vnc.env (wait up to 10s for it to appear)
    for i in $(seq 1 10); do
        # Try direct read first, then sudo (TCC may block direct access)
        VNC_ENV_CONTENT=""
        if VNC_ENV_CONTENT=$(cat "$LUME_CONFIG" 2>/dev/null) && [ -n "$VNC_ENV_CONTENT" ]; then
            true
        elif VNC_ENV_CONTENT=$(sudo -n cat "$LUME_CONFIG" 2>/dev/null) && [ -n "$VNC_ENV_CONTENT" ]; then
            true
        fi
        if [ -n "$VNC_ENV_CONTENT" ]; then
            eval "$VNC_ENV_CONTENT"
            LUME_VNC_PORT="${VNC_PORT:-}"
            LUME_VNC_PASSWORD="${VNC_PASSWORD:-}"
            echo "Read VNC config from VirtioFS: port=$LUME_VNC_PORT" >> "$LOG_FILE"
            # Cache locally for future restarts
            echo "$VNC_ENV_CONTENT" > "$LOCAL_VNC_ENV"
            break
        fi
        sleep 1
    done
fi

if [ -z "$LUME_VNC_PORT" ]; then
    echo "WARNING: No VNC config found in ~/.vnc.env or VirtioFS. VNC backend may not work." >> "$LOG_FILE"
    echo "Lume should auto-deliver VNC config via SSH. Check that the VM was started with 'lume run'." >> "$LOG_FILE"
fi

# Set VNC backend env vars — prefer discovered values
export CUA_BACKEND=vnc
export CUA_VNC_HOST="$GATEWAY_IP"
export CUA_VNC_PORT="${LUME_VNC_PORT:-5900}"
export CUA_VNC_PASSWORD="${LUME_VNC_PASSWORD:-}"

# Update packages
echo "Updating cua-agent..." >> "$LOG_FILE"
pip install --upgrade --no-input "cua-agent[all]" >> "$LOG_FILE" 2>&1 || true

echo "Updating cua-computer-server..." >> "$LOG_FILE"
pip install --upgrade --no-input "cua-computer-server[vnc]" >> "$LOG_FILE" 2>&1 || true

# Ensure Playwright Firefox
echo "Ensuring Playwright Firefox..." >> "$LOG_FILE"
pip install --upgrade --no-input playwright >> "$LOG_FILE" 2>&1 || true
playwright install firefox >> "$LOG_FILE" 2>&1 || true

# Start server with VNC backend pointing at lume's VNC on the host
echo "Starting CUA computer server (vnc backend) on port __PORT__..." >> "$LOG_FILE"
echo "  VNC target: $CUA_VNC_HOST:$CUA_VNC_PORT" >> "$LOG_FILE"
python -m computer_server --port __PORT__ >> "$LOG_FILE" 2>&1
SCRIPT_EOF

    # Stamp the server port
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
        <key>CUA_BACKEND</key>
        <string>vnc</string>
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
# Phase 7: System Configuration + Auto-Login
# ============================================

configure_system() {
    print_phase "Phase 7: System Configuration"

    if ! confirm "Disable screen lock, screensaver, sleep, and enable auto-login?"; then
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

    # Configure auto-login (ensures GUI session + unlocked keychain on boot)
    echo "Configuring auto-login for $CURRENT_USER..."

    if [ "$AUTO_YES" = true ]; then
        sudo -A defaults write /Library/Preferences/com.apple.loginwindow autoLoginUser "$CURRENT_USER"
    else
        sudo defaults write /Library/Preferences/com.apple.loginwindow autoLoginUser "$CURRENT_USER"
    fi

    # Create kcpassword file (XOR-encrypted password for auto-login)
    python3 << PYEOF
password = "$VM_PASSWORD"
key = bytes.fromhex("7d895223d2bcddeaa3b91f")
padded_len = ((len(password) + 11) // 12) * 12
padded = password + "\x00" * (padded_len - len(password))
result = bytearray()
for i, char in enumerate(padded.encode("utf-8")):
    result.append(char ^ key[i % len(key)])
with open("/tmp/kcpassword", "wb") as f:
    f.write(result)
PYEOF

    if [ "$AUTO_YES" = true ]; then
        sudo -A mv /tmp/kcpassword /etc/kcpassword
        sudo -A chown root:wheel /etc/kcpassword
        sudo -A chmod 600 /etc/kcpassword
    else
        sudo mv /tmp/kcpassword /etc/kcpassword
        sudo chown root:wheel /etc/kcpassword
        sudo chmod 600 /etc/kcpassword
    fi

    echo -e "${GREEN}System configured (no sleep, no screensaver, full keyboard nav, auto-login)${NC}"
}

# ============================================
# Phase 8: Start Services
# ============================================

start_services() {
    print_phase "Phase 8: Start Services"

    ensure_brew

    # Start supervisor (manages websockify)
    if command -v brew &>/dev/null; then
        echo "Starting supervisor..."
        brew services start supervisor 2>/dev/null || true
        sleep 3

        if command -v supervisorctl &>/dev/null; then
            supervisorctl reread 2>/dev/null || true
            supervisorctl update 2>/dev/null || true
        fi
    fi

    echo -e "${GREEN}Services started${NC}"
}

# ============================================
# Phase 9: Verification
# ============================================

verify() {
    print_phase "Verification"

    echo "System:"
    echo "  User:    $CURRENT_USER"
    echo "  Gateway: $GATEWAY_IP"
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

    # noVNC / websockify
    if command -v supervisorctl &>/dev/null && supervisorctl status websockify 2>/dev/null | grep -q RUNNING; then
        echo -e "  ${GREEN}✓${NC} noVNC + websockify (port $NOVNC_PORT -> lume VNC)"
    elif [ -d "/usr/local/share/novnc" ]; then
        echo -e "  ${YELLOW}~${NC} noVNC installed (websockify may not be running yet)"
    else
        echo -e "  ${RED}✗${NC} noVNC"
    fi

    # Auto-login
    if defaults read /Library/Preferences/com.apple.loginwindow autoLoginUser 2>/dev/null | grep -q "$CURRENT_USER"; then
        echo -e "  ${GREEN}✓${NC} Auto-login ($CURRENT_USER)"
    else
        echo -e "  ${YELLOW}~${NC} Auto-login (may need reboot)"
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
    echo "Services:"
    echo "  CUA server:  port $CUA_SERVER_PORT (starts at login via LaunchAgent)"
    echo "  noVNC:       port $NOVNC_PORT -> lume VNC (auto-configured via ~/.vnc.env)"
    echo ""
    echo "VNC config is auto-delivered by lume. Just run:"
    echo "  lume run <vm-name>"
    echo ""
    echo "To start CUA server now (from the host):"
    echo "  lume ssh <vm-name> 'launchctl load ~/Library/LaunchAgents/com.trycua.computer_server.plist'"
    echo ""
    echo "To test from the host:"
    echo "  curl http://<vm-ip>:$CUA_SERVER_PORT/status"
    echo ""
    echo "To access noVNC from the host network:"
    echo "  http://<vm-ip>:$NOVNC_PORT/vnc.html"
    echo ""
    echo "To access noVNC remotely (from outside the host):"
    echo "  1. SSH tunnel:  ssh -L $NOVNC_PORT:<vm-ip>:$NOVNC_PORT user@host"
    echo "  2. Open:        http://localhost:$NOVNC_PORT/vnc.html?host=localhost&port=$NOVNC_PORT"
    echo ""
    echo "NOTE: A reboot is recommended for auto-login to take effect."
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
echo -e " ⠀⠀⠀⠀⠀⠉⠛⠁⠈⠿⣿⣿⣿⣷⣄⣠⡼⠟⠁${NC}  ${BLUE}CUA Setup${NC}"
echo -e "${BLUE}           Make a Lume VM cua-compatible${NC}"
echo ""

install_homebrew
install_python
install_system_deps
setup_cua_server
setup_novnc
create_startup_script
configure_system
start_services
verify

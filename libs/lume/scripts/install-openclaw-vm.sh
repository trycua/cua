#!/bin/bash
#
# install-openclaw-vm.sh
# Provisions OpenClaw on a Lume macOS VM
#
# This script runs ON the VM itself (not the host).
# It is idempotent and interactive — it checks what's already installed
# and asks before installing anything new.
#
# Usage: ./install-openclaw-vm.sh [--yes] [anthropic-api-key] [gateway-port]
#
# Prerequisites:
#   - macOS VM with SSH enabled (created via lume setup --unattended)
#

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# Parse --yes flag
AUTO_YES=false
if [ "${1:-}" = "--yes" ] || [ "${1:-}" = "-y" ]; then
    AUTO_YES=true
    shift
fi

GATEWAY_PORT="${2:-18789}"
VM_PASSWORD="${VM_PASSWORD:-lume}"
OPENCLAW_DIR="$HOME/openclaw"
REPO_URL="https://github.com/openclaw/openclaw.git"

# Helper: print phase header
print_phase() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}  $1${NC}"
    echo -e "${BLUE}========================================${NC}"
    echo ""
}

# Helper: prompt user (default yes, auto-yes with --yes flag)
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

# Helper: ensure brew is in PATH for this session
ensure_brew() {
    if [ -f /opt/homebrew/bin/brew ]; then
        eval "$(/opt/homebrew/bin/brew shellenv)"
    fi
}

# Helper: ensure pnpm is in PATH for this session
ensure_pnpm() {
    ensure_brew
    local pnpm_home="$HOME/Library/pnpm"
    if [ -d "$pnpm_home" ]; then
        export PNPM_HOME="$pnpm_home"
        export PATH="$PNPM_HOME:$PATH"
    fi
    # Also source shell config in case pnpm added itself there
    source "$HOME/.zshrc" 2>/dev/null || true
}

# ============================================
# Get Anthropic API Key
# ============================================
ANTHROPIC_API_KEY="${1:-}"
if [ -z "$ANTHROPIC_API_KEY" ]; then
    echo -e "${YELLOW}No Anthropic API key provided as argument.${NC}"
    read -r -p "Enter your Anthropic API key: " ANTHROPIC_API_KEY
    if [ -z "$ANTHROPIC_API_KEY" ]; then
        echo -e "${RED}Error: Anthropic API key is required${NC}"
        exit 1
    fi
fi

# ============================================
# Phase 1: Prerequisites
# ============================================
print_phase "Phase 1: Prerequisites"

# Homebrew
if command -v brew &>/dev/null; then
    echo -e "${GREEN}Homebrew already installed$(brew --version | head -1 | sed 's/Homebrew //' | sed 's/^/ (v/')${NC})"
else
    if confirm "Install Homebrew?"; then
        # Prime sudo so the Homebrew installer doesn't fail
        if [ "$AUTO_YES" = true ]; then
            # Create an askpass helper so all sudo calls during install work without a TTY
            ASKPASS_SCRIPT=$(mktemp)
            printf '#!/bin/bash\necho "%s"\n' "$VM_PASSWORD" > "$ASKPASS_SCRIPT"
            chmod +x "$ASKPASS_SCRIPT"
            export SUDO_ASKPASS="$ASKPASS_SCRIPT"
            sudo -A -v
            NONINTERACTIVE=1 SUDO_ASKPASS="$ASKPASS_SCRIPT" /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
            rm -f "$ASKPASS_SCRIPT"
        else
            echo -e "${YELLOW}Homebrew requires sudo access. Enter your password if prompted.${NC}"
            sudo -v
            NONINTERACTIVE=1 /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
        fi
        echo 'eval "$(/opt/homebrew/bin/brew shellenv)"' >> "$HOME/.zprofile"
    else
        echo -e "${YELLOW}Skipping Homebrew. Some later steps may fail.${NC}"
    fi
fi
ensure_brew

# Node.js
if command -v node &>/dev/null; then
    echo -e "${GREEN}Node.js already installed ($(node --version))${NC}"
else
    if confirm "Install Node.js 22?"; then
        brew install node@22
        # node@22 is keg-only, add to PATH
        echo 'export PATH="/opt/homebrew/opt/node@22/bin:$PATH"' >> "$HOME/.zshrc"
        export PATH="/opt/homebrew/opt/node@22/bin:$PATH"
    else
        echo -e "${YELLOW}Skipping Node.js.${NC}"
    fi
fi
# Ensure keg-only node@22 is in PATH
if [ -d /opt/homebrew/opt/node@22/bin ]; then
    export PATH="/opt/homebrew/opt/node@22/bin:$PATH"
fi

# pnpm
if command -v pnpm &>/dev/null; then
    echo -e "${GREEN}pnpm already installed ($(pnpm --version))${NC}"
else
    if confirm "Install pnpm?"; then
        brew install pnpm
        pnpm setup
        ensure_pnpm
    else
        echo -e "${YELLOW}Skipping pnpm.${NC}"
    fi
fi
ensure_pnpm

# ============================================
# Phase 2: Clone & Build OpenClaw
# ============================================
print_phase "Phase 2: OpenClaw"

if [ -d "$OPENCLAW_DIR" ]; then
    echo -e "${GREEN}OpenClaw repo already exists at $OPENCLAW_DIR${NC}"
    if confirm "Pull latest changes and rebuild?"; then
        cd "$OPENCLAW_DIR"
        git pull
        pnpm install
        pnpm build
    fi
else
    if confirm "Clone and build OpenClaw?"; then
        git clone "$REPO_URL" "$OPENCLAW_DIR"
        cd "$OPENCLAW_DIR"
        pnpm install
        pnpm build
    else
        echo -e "${YELLOW}Skipping OpenClaw. Later steps will fail without it.${NC}"
    fi
fi

# ============================================
# Phase 3: Onboarding
# ============================================
print_phase "Phase 3: Onboarding"

if confirm "Run OpenClaw onboarding?"; then
    cd "$OPENCLAW_DIR"
    pnpm openclaw onboard --install-daemon --non-interactive --accept-risk --anthropic-api-key "$ANTHROPIC_API_KEY"
else
    echo -e "${YELLOW}Skipping onboarding.${NC}"
fi

# ============================================
# Phase 4: Additional Tools
# ============================================
print_phase "Phase 4: Additional Tools"

ensure_brew

# GitHub CLI (install before Claude Code CLI since brew tap may need it)
if command -v gh &>/dev/null; then
    echo -e "${GREEN}GitHub CLI already installed ($(gh --version | head -1))${NC}"
else
    if confirm "Install GitHub CLI?"; then
        brew install gh
    else
        echo -e "${YELLOW}Skipping GitHub CLI.${NC}"
    fi
fi

# Claude Code CLI
if command -v claude &>/dev/null; then
    echo -e "${GREEN}Claude Code CLI already installed${NC}"
else
    if confirm "Install Claude Code CLI?"; then
        brew tap anthropics/tap 2>/dev/null || true
        brew install claude-code
    else
        echo -e "${YELLOW}Skipping Claude Code CLI.${NC}"
    fi
fi

# Peekaboo
if command -v peekaboo &>/dev/null; then
    echo -e "${GREEN}Peekaboo already installed${NC}"
else
    if confirm "Install Peekaboo (screen capture)?"; then
        brew install steipete/tap/peekaboo
    else
        echo -e "${YELLOW}Skipping Peekaboo.${NC}"
    fi
fi

# ============================================
# Phase 5: Configure & Start
# ============================================
print_phase "Phase 5: Configure & Start"

if confirm "Configure security settings and start gateway?"; then
    cd "$OPENCLAW_DIR"
    pnpm openclaw config set channels.whatsapp.dmPolicy allowlist 2>/dev/null || echo -e "${YELLOW}Config update skipped${NC}"

    # Start gateway if the plist exists (created by onboarding --install-daemon)
    GATEWAY_PLIST="$HOME/Library/LaunchAgents/ai.openclaw.gateway.plist"
    if [ -f "$GATEWAY_PLIST" ]; then
        launchctl bootstrap gui/$(id -u) "$GATEWAY_PLIST" 2>/dev/null \
            || launchctl kickstart -k "gui/$(id -u)/ai.openclaw.gateway"
        echo -e "${GREEN}Gateway started${NC}"
    else
        echo -e "${YELLOW}Gateway plist not found at $GATEWAY_PLIST${NC}"
        echo -e "${YELLOW}Run onboarding with --install-daemon first to create it.${NC}"
    fi
else
    echo -e "${YELLOW}Skipping configuration and gateway start.${NC}"
fi

# ============================================
# Phase 6: Verification
# ============================================
print_phase "Verification"

ensure_pnpm

if [ -d "$OPENCLAW_DIR" ]; then
    VERSION=$(cd "$OPENCLAW_DIR" && node openclaw.mjs --version 2>/dev/null || echo "unknown")
    echo -e "OpenClaw version: ${GREEN}$VERSION${NC}"
fi

DAEMON_STATUS=$(launchctl list 2>/dev/null | grep openclaw || echo "not running")
echo -e "Daemon status:    ${GREEN}$DAEMON_STATUS${NC}"

echo ""
echo -e "${GREEN}========================================${NC}"
echo -e "${GREEN}  Setup Complete!${NC}"
echo -e "${GREEN}========================================${NC}"
echo ""
echo -e "Gateway Port: ${BLUE}$GATEWAY_PORT${NC}"
echo ""
echo "To access the dashboard from the host:"
echo -e "  ${YELLOW}lume ssh <vm-name> -L $GATEWAY_PORT:127.0.0.1:$GATEWAY_PORT${NC}"
echo "  Then open: http://127.0.0.1:$GATEWAY_PORT"
echo ""

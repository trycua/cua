#!/bin/bash
set -e

# Lumier Installer
# This script installs Lumier to your system

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

# Script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Parse command line arguments
while [ "$#" -gt 0 ]; do
  case "$1" in
    --install-dir=*)
      INSTALL_DIR="${1#*=}"
      ;;
    --help)
      echo "${BOLD}${BLUE}Lumier Installer${NORMAL}"
      echo "Usage: $0 [OPTIONS]"
      echo ""
      echo "Options:"
      echo "  --install-dir=DIR   Install to the specified directory (default: $DEFAULT_INSTALL_DIR)"
      echo "  --help              Display this help message"
      echo ""
      echo "Examples:"
      echo "  $0                               # Install to $DEFAULT_INSTALL_DIR"
      echo "  $0 --install-dir=/usr/local/bin  # Install to system directory (may require root privileges)"
      echo "  INSTALL_DIR=/opt/lumier $0       # Install to /opt/lumier (legacy env var support)"
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

echo "${BOLD}${BLUE}Lumier Installer${NORMAL}"
echo "This script will install Lumier to your system."

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
    echo "${RED}Error: Lumier only supports macOS on Apple Silicon (ARM64).${NORMAL}"
    exit 1
  fi
  
  PLATFORM="darwin-arm64"
  echo "Detected platform: ${BOLD}$PLATFORM${NORMAL}"
}

# Check dependencies
check_dependencies() {
  echo "Checking dependencies..."
  
  # Check if lume is installed
  if ! command -v lume &> /dev/null; then
    echo "${RED}Error: Lume is required but not installed.${NORMAL}"
    echo "Please install Lume first: https://github.com/trycua/cua/blob/main/libs/lume/README.md"
    exit 1
  fi
  
  # Check if socat is installed
  if ! command -v socat &> /dev/null; then
    echo "${YELLOW}Warning: socat is required but not installed.${NORMAL}"
    echo "Installing socat with Homebrew..."
    
    # Check if Homebrew is installed
    if ! command -v brew &> /dev/null; then
      echo "${RED}Error: Homebrew is required to install socat.${NORMAL}"
      echo "Please install Homebrew first: https://brew.sh/"
      echo "Or install socat manually, then run this script again."
      exit 1
    fi
    
    # Install socat
    brew install socat
  fi
  
  # Check if Docker is installed
  if ! command -v docker &> /dev/null; then
    echo "${YELLOW}Warning: Docker is required but not installed.${NORMAL}"
    echo "Please install Docker: https://docs.docker.com/get-docker/"
    echo "Continuing with installation, but Lumier will not work without Docker."
  fi
  
  echo "${GREEN}All dependencies are satisfied.${NORMAL}"
}

# Copy the lumier script directly
copy_lumier() {
  echo "Copying lumier script to $INSTALL_DIR..."
  cp "$SCRIPT_DIR/lumier" "$INSTALL_DIR/lumier"
  chmod +x "$INSTALL_DIR/lumier"
}

# Main installation flow
main() {
  check_permissions
  detect_platform
  check_dependencies
  
  echo "Installing Lumier to $INSTALL_DIR..."
  
  # Create install directory if it doesn't exist
  mkdir -p "$INSTALL_DIR"
  
  # Copy the lumier script
  copy_lumier
  
  echo "${GREEN}Installation complete!${NORMAL}"
  echo "Lumier has been installed to ${BOLD}$INSTALL_DIR/lumier${NORMAL}"
  
  # Check if the installation directory is in PATH
  if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
    echo "${YELLOW}Warning: $INSTALL_DIR is not in your PATH.${NORMAL}"
    echo "To add it, run one of these commands based on your shell:"
    echo "  For bash: echo 'export PATH=\"\$PATH:$INSTALL_DIR\"' >> ~/.bash_profile"
    echo "  For zsh:  echo 'export PATH=\"\$PATH:$INSTALL_DIR\"' >> ~/.zshrc"
    echo "  For fish: echo 'fish_add_path $INSTALL_DIR' >> ~/.config/fish/config.fish"
  fi
}

# Run the installation
main 
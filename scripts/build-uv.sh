#!/bin/bash

# Exit on error
set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Function to print step information
print_step() {
    echo -e "${BLUE}==> $1${NC}"
}

# Function to print success message
print_success() {
    echo -e "${GREEN}==> Success: $1${NC}"
}

# Function to print error message
print_error() {
    echo -e "${RED}==> Error: $1${NC}" >&2
}

# Function to print warning message
print_warning() {
    echo -e "${YELLOW}==> Warning: $1${NC}"
}

# Function to check if UV is installed
check_uv() {
    if command -v uv &> /dev/null; then
        print_success "UV is already installed"
        uv --version
        return 0
    else
        return 1
    fi
}

# Function to install UV
install_uv() {
    print_step "UV not found. Installing UV..."
    
    # Detect OS
    if [[ "$OSTYPE" == "linux-gnu"* ]] || [[ "$OSTYPE" == "darwin"* ]]; then
        print_step "Installing UV for Unix-like system..."
        curl -LsSf https://astral.sh/uv/install.sh | sh
        
        # Add UV to PATH for current session
        export PATH="$HOME/.cargo/bin:$PATH"
        
        # Check if installation was successful
        if command -v uv &> /dev/null; then
            print_success "UV installed successfully"
            uv --version
        else
            print_error "UV installation failed"
            print_step "Please restart your terminal and try again, or install manually:"
            echo "  curl -LsSf https://astral.sh/uv/install.sh | sh"
            exit 1
        fi
    elif [[ "$OSTYPE" == "msys" ]] || [[ "$OSTYPE" == "cygwin" ]]; then
        print_error "For Windows, please use PowerShell and run:"
        echo "  powershell -ExecutionPolicy ByPass -c \"irm https://astral.sh/uv/install.ps1 | iex\""
        exit 1
    else
        print_error "Unsupported operating system: $OSTYPE"
        print_step "Please install UV manually from: https://docs.astral.sh/uv/getting-started/installation/"
        exit 1
    fi
}

# Get the script's directory
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
PROJECT_ROOT="$( cd "${SCRIPT_DIR}/.." && pwd )"

# Change to project root
cd "$PROJECT_ROOT"

# Check if UV is installed, install if not
if ! check_uv; then
    install_uv
fi

# Load environment variables from .env.local
if [ -f .env.local ]; then
    print_step "Loading environment variables from .env.local..."
    set -a
    source .env.local
    set +a
    print_success "Environment variables loaded"
else
    print_error ".env.local file not found"
    exit 1
fi

# Clean up existing environments and cache
print_step "Cleaning up existing environments..."
find . -type d -name "__pycache__" -exec rm -rf {} + 2>/dev/null || true
find . -type d -name ".pytest_cache" -exec rm -rf {} + 2>/dev/null || true
find . -type d -name "dist" -exec rm -rf {} + 2>/dev/null || true
find . -type d -name ".venv" -exec rm -rf {} + 2>/dev/null || true
find . -type d -name "*.egg-info" -exec rm -rf {} + 2>/dev/null || true
print_success "Environment cleanup complete"

# Install Python 3.12 using UV
print_step "Installing Python 3.12 using UV..."
uv python install 3.12
print_success "Python 3.12 installed"

# Create virtual environment using UV
print_step "Creating virtual environment with UV..."
uv venv .venv --python 3.12
print_success "Virtual environment created"

# Activate virtual environment
print_step "Activating virtual environment..."
source .venv/bin/activate
print_success "Virtual environment activated"

# Function to install a package and its dependencies using UV
install_package() {
    local package_dir=$1
    local package_name=$2
    local extras=$3
    print_step "Installing ${package_name} with UV..."
    cd "$package_dir"
    
    if [ -f "pyproject.toml" ]; then
        if [ -n "$extras" ]; then
            uv pip install -e ".[${extras}]"
        else
            uv pip install -e .
        fi
    else
        print_error "No pyproject.toml found in ${package_dir}"
        return 1
    fi
    
    cd "$PROJECT_ROOT"
}

# Install packages in order of dependency
print_step "Installing packages in development mode with UV..."

# Install core first (base package with telemetry support)
install_package "libs/python/core" "core"

# Install computer with all its dependencies and extras
install_package "libs/python/computer" "computer" "all"

# Install omniparser
install_package "libs/python/som" "som"

# Install agent with all its dependencies and extras
install_package "libs/python/agent" "agent" "all"

# Install computer-server
install_package "libs/python/computer-server" "computer-server"

# Install mcp-server
install_package "libs/python/mcp-server" "mcp-server"

# Install development tools from root project
print_step "Installing development dependencies with UV..."
uv pip install -e ".[dev,test,docs]"

# Create a .env file for VS Code to use the virtual environment
print_step "Creating .env file for VS Code..."
echo "PYTHONPATH=${PROJECT_ROOT}/libs/python/core:${PROJECT_ROOT}/libs/python/computer:${PROJECT_ROOT}/libs/python/agent:${PROJECT_ROOT}/libs/python/som:${PROJECT_ROOT}/libs/python/computer-server:${PROJECT_ROOT}/libs/python/mcp-server" > .env

print_success "All packages installed successfully with UV!"
print_step "Your virtual environment is ready. To activate it:"
echo "  source .venv/bin/activate"
print_step "UV provides fast dependency resolution and installation."
print_step "You can also use 'uv run' to run commands in the virtual environment without activation."

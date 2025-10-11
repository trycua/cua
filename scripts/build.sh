#!/bin/bash

# Exit on error
set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
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

# Get the script's directory
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
PROJECT_ROOT="$( cd "${SCRIPT_DIR}/.." && pwd )"

# Change to project root
cd "$PROJECT_ROOT"

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
find . -type d -name "__pycache__" -exec rm -rf {} +
find . -type d -name ".pytest_cache" -exec rm -rf {} +
find . -type d -name "dist" -exec rm -rf {} +
find . -type d -name ".venv" -exec rm -rf {} +
find . -type d -name "*.egg-info" -exec rm -rf {} +
print_success "Environment cleanup complete"

# Create and activate virtual environment
print_step "Creating virtual environment..."
python -m venv .venv
source .venv/bin/activate

# Upgrade pip and install build tools
print_step "Upgrading pip and installing build tools..."
python -m pip install --upgrade pip setuptools wheel

# Function to install a package and its dependencies
install_package() {
    local package_dir=$1
    local package_name=$2
    local extras=$3
    print_step "Installing ${package_name}..."
    cd "$package_dir"
    
    if [ -f "pyproject.toml" ]; then
        if [ -n "$extras" ]; then
            pip install -e ".[${extras}]"
        else
            pip install -e .
        fi
    else
        print_error "No pyproject.toml found in ${package_dir}"
        return 1
    fi
    
    cd "$PROJECT_ROOT"
}

# Install packages in order of dependency
print_step "Installing packages in development mode..."

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
print_step "Installing development dependencies..."
pip install -e ".[dev,test,docs]"

# Create a .env file for VS Code to use the virtual environment
print_step "Creating .env file for VS Code..."
echo "PYTHONPATH=${PROJECT_ROOT}/libs/python/core:${PROJECT_ROOT}/libs/python/computer:${PROJECT_ROOT}/libs/python/agent:${PROJECT_ROOT}/libs/python/som:${PROJECT_ROOT}/libs/python/computer-server:${PROJECT_ROOT}/libs/python/mcp-server" > .env

print_success "All packages installed successfully!"
print_step "Your virtual environment is ready. To activate it:"
echo "  source .venv/bin/activate"

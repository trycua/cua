#!/bin/bash

set -e

# Colors for output
GREEN='\033[0;32m'
BLUE='\033[0;34m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Print with color
print_info() {
    echo -e "${BLUE}==> $1${NC}"
}

print_success() {
    echo -e "${GREEN}==> $1${NC}"
}

print_error() {
    echo -e "${RED}==> $1${NC}"
}

print_warning() {
    echo -e "${YELLOW}==> $1${NC}"
}

echo "🚀 Launching Cua Computer-Use Agent UI..."

# Check if Docker is installed
if ! command -v docker &> /dev/null; then
    print_error "Docker is not installed!"
    echo ""
    echo "To use Cua with Docker containers, you need to install Docker first:"
    echo ""
    echo "📦 Install Docker:"
    echo "  • macOS: Download Docker Desktop from https://docker.com/products/docker-desktop"
    echo "  • Windows: Download Docker Desktop from https://docker.com/products/docker-desktop"
    echo "  • Linux: Follow instructions at https://docs.docker.com/engine/install/"
    echo ""
    echo "After installing Docker, run this script again."
    exit 1
fi

# Check if Docker daemon is running
if ! docker info &> /dev/null; then
    print_error "Docker is installed but not running!"
    echo ""
    echo "Please start Docker Desktop and try again."
    exit 1
fi

print_success "Docker is installed and running!"

# Save the original working directory
ORIGINAL_DIR="$(pwd)"

DEMO_DIR="$HOME/.cua"
mkdir -p "$DEMO_DIR"


# Check if we're already in the Cua repository
# Look for the specific trycua identifier in pyproject.toml
if [[ -f "pyproject.toml" ]] && grep -q "gh@trycua.com" "pyproject.toml"; then
  print_success "Already in Cua repository - using current directory"
  REPO_DIR="$ORIGINAL_DIR"
  USE_EXISTING_REPO=true
else
  # Directories used by the script when not in repo
  REPO_DIR="$DEMO_DIR/cua"
  USE_EXISTING_REPO=false
fi

# Function to clean up on exit
cleanup() {
  cd "$ORIGINAL_DIR" 2>/dev/null || true
}
trap cleanup EXIT

echo ""
echo "Choose your Cua setup:"
echo "1) ☁️  Cua Cloud Sandbox (works on any system)"
echo "2) 🖥️  Local macOS VMs (requires Apple Silicon Mac + macOS 15+)"
echo "3) 🖥️  Local Windows VMs (requires Windows 10 / 11)"
echo ""
read -p "Enter your choice (1, 2, or 3): " CHOICE

if [[ "$CHOICE" == "1" ]]; then
  # Cua Cloud Sandbox setup
  echo ""
  print_info "Setting up Cua Cloud Sandbox..."
  echo ""
  
  # Check if existing .env.local already has CUA_API_KEY
  REPO_ENV_FILE="$REPO_DIR/.env.local"
  CURRENT_ENV_FILE="$ORIGINAL_DIR/.env.local"
  
  CUA_API_KEY=""
  
  # First check current directory
  if [[ -f "$CURRENT_ENV_FILE" ]] && grep -q "CUA_API_KEY=" "$CURRENT_ENV_FILE"; then
    EXISTING_CUA_KEY=$(grep "CUA_API_KEY=" "$CURRENT_ENV_FILE" | cut -d'=' -f2- | tr -d '"' | tr -d "'" | xargs)
    if [[ -n "$EXISTING_CUA_KEY" && "$EXISTING_CUA_KEY" != "your_cua_api_key_here" && "$EXISTING_CUA_KEY" != "" ]]; then
      CUA_API_KEY="$EXISTING_CUA_KEY"
    fi
  fi
  
  # Then check repo directory if not found in current dir
  if [[ -z "$CUA_API_KEY" ]] && [[ -f "$REPO_ENV_FILE" ]] && grep -q "CUA_API_KEY=" "$REPO_ENV_FILE"; then
    EXISTING_CUA_KEY=$(grep "CUA_API_KEY=" "$REPO_ENV_FILE" | cut -d'=' -f2- | tr -d '"' | tr -d "'" | xargs)
    if [[ -n "$EXISTING_CUA_KEY" && "$EXISTING_CUA_KEY" != "your_cua_api_key_here" && "$EXISTING_CUA_KEY" != "" ]]; then
      CUA_API_KEY="$EXISTING_CUA_KEY"
    fi
  fi
  
  # If no valid API key found, prompt for one
  if [[ -z "$CUA_API_KEY" ]]; then
    echo "To use Cua Cloud Sandbox, you need to:"
    echo "1. Sign up at https://cua.ai"
    echo "2. Create a Cloud Sandbox"
    echo "3. Generate an Api Key"
    echo ""
    read -p "Enter your Cua Api Key: " CUA_API_KEY
    
    if [[ -z "$CUA_API_KEY" ]]; then
      print_error "Cua Api Key is required for Cloud Sandbox."
      exit 1
    fi
  else
    print_success "Found existing CUA API key"
  fi
  
  USE_CLOUD=true
  COMPUTER_TYPE="cloud"

elif [[ "$CHOICE" == "2" ]]; then
  # Local macOS VM setup
  echo ""
  print_info "Setting up local macOS VMs..."
  
  # Check for Apple Silicon Mac
  if [[ $(uname -s) != "Darwin" || $(uname -m) != "arm64" ]]; then
    print_error "Local macOS VMs require an Apple Silicon Mac (M1/M2/M3/M4)."
    echo "💡 Consider using Cua Cloud Sandbox instead (option 1)."
    exit 1
  fi

  # Check for macOS 15 (Sequoia) or newer
  OSVERSION=$(sw_vers -productVersion)
  if [[ $(echo "$OSVERSION 15.0" | tr " " "\n" | sort -V | head -n 1) != "15.0" ]]; then
    print_error "Local macOS VMs require macOS 15 (Sequoia) or newer. You have $OSVERSION."
    echo "💡 Consider using Cua Cloud Sandbox instead (option 1)."
    exit 1
  fi

  USE_CLOUD=false
  COMPUTER_TYPE="macos"

elif [[ "$CHOICE" == "3" ]]; then
  # Local Windows VM setup
  echo ""
  print_info "Setting up local Windows VMs..."
  
  # Check if we're on Windows
  if [[ $(uname -s) != MINGW* && $(uname -s) != CYGWIN* && $(uname -s) != MSYS* ]]; then
    print_error "Local Windows VMs require Windows 10 or 11."
    echo "💡 Consider using Cua Cloud Sandbox instead (option 1)."
    echo ""
    echo "🔗 If you are using WSL, refer to the blog post to get started: https://cua.ai/blog/windows-sandbox"
    exit 1
  fi

  USE_CLOUD=false
  COMPUTER_TYPE="windows"

else
  print_error "Invalid choice. Please run the script again and choose 1, 2, or 3."
  exit 1
fi

print_success "All checks passed! 🎉"

# Create demo directory and handle repository
if [[ "$USE_EXISTING_REPO" == "true" ]]; then
  print_info "Using existing repository in current directory"
  cd "$REPO_DIR"
else  
  # Clone or update the repository
  if [[ ! -d "$REPO_DIR" ]]; then
    print_info "Cloning Cua repository..."
    cd "$DEMO_DIR"
    git clone https://github.com/trycua/cua.git
  else
    print_info "Updating Cua repository..."
    cd "$REPO_DIR"
    git pull origin main
  fi
  
  cd "$REPO_DIR"
fi

# Create .env.local file with API keys
ENV_FILE="$REPO_DIR/.env.local"
if [[ ! -f "$ENV_FILE" ]]; then
  cat > "$ENV_FILE" << EOF
# Uncomment and add your API keys here
# OPENAI_API_KEY=your_openai_api_key_here
# ANTHROPIC_API_KEY=your_anthropic_api_key_here
CUA_API_KEY=your_cua_api_key_here
EOF
  print_success "Created .env.local file with API key placeholders"
else
  print_success "Found existing .env.local file - keeping your current settings"
fi

if [[ "$USE_CLOUD" == "true" ]]; then
  # Add CUA API key to .env.local if not already present
  if ! grep -q "CUA_API_KEY" "$ENV_FILE"; then
    echo "CUA_API_KEY=$CUA_API_KEY" >> "$ENV_FILE"
    print_success "Added CUA_API_KEY to .env.local"
  elif grep -q "CUA_API_KEY=your_cua_api_key_here" "$ENV_FILE"; then
    # Update placeholder with actual key
    sed -i.bak "s/CUA_API_KEY=your_cua_api_key_here/CUA_API_KEY=$CUA_API_KEY/" "$ENV_FILE"
    print_success "Updated CUA_API_KEY in .env.local"
  fi
fi

# Build the Docker image if it doesn't exist
print_info "Checking Docker image..."
if ! docker image inspect cua-dev-image &> /dev/null; then
  print_info "Building Docker image (this may take a while)..."
  ./scripts/run-docker-dev.sh build
else
  print_success "Docker image already exists"
fi

# Install Lume if needed for local VMs
if [[ "$USE_CLOUD" == "false" && "$COMPUTER_TYPE" == "macos" ]]; then
  if ! command -v lume &> /dev/null; then
    print_info "Installing Lume CLI..."
    curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/lume/scripts/install.sh | bash
    
    # Add lume to PATH for this session if it's not already there
    if ! command -v lume &> /dev/null; then
      export PATH="$PATH:$HOME/.local/bin"
    fi
  fi

  # Pull the macOS CUA image if not already present
  if ! lume ls | grep -q "macos-sequoia-cua"; then
    # Check available disk space
    IMAGE_SIZE_GB=30
    AVAILABLE_SPACE_KB=$(df -k $HOME | tail -1 | awk '{print $4}')
    AVAILABLE_SPACE_GB=$(($AVAILABLE_SPACE_KB / 1024 / 1024))
    
    echo "📊 The macOS CUA image will use approximately ${IMAGE_SIZE_GB}GB of disk space."
    echo "   You currently have ${AVAILABLE_SPACE_GB}GB available on your system."
    
    # Prompt for confirmation
    read -p "   Continue? [y]/n: " CONTINUE
    CONTINUE=${CONTINUE:-y}
    
    if [[ $CONTINUE =~ ^[Yy]$ ]]; then
      print_info "Pulling macOS CUA image (this may take a while)..."
      
      # Use caffeinate on macOS to prevent system sleep during the pull
      if command -v caffeinate &> /dev/null; then
        print_info "Using caffeinate to prevent system sleep during download..."
        caffeinate -i lume pull macos-sequoia-cua:latest
      else
        lume pull macos-sequoia-cua:latest
      fi
    else
      print_error "Installation cancelled."
      exit 1
    fi
  fi

  # Check if the VM is running
  print_info "Checking if the macOS CUA VM is running..."
  VM_RUNNING=$(lume ls | grep "macos-sequoia-cua" | grep "running" || echo "")

  if [ -z "$VM_RUNNING" ]; then
    print_info "Starting the macOS CUA VM in the background..."
    lume run macos-sequoia-cua:latest &
    # Wait a moment for the VM to initialize
    sleep 5
    print_success "VM started successfully."
  else
    print_success "macOS CUA VM is already running."
  fi
fi

# Create a convenience script to run the demo
cat > "$DEMO_DIR/start_ui.sh" << EOF
#!/bin/bash
cd "$REPO_DIR"
./scripts/run-docker-dev.sh run agent_ui_examples.py
EOF
chmod +x "$DEMO_DIR/start_ui.sh"

print_success "Setup complete!"

if [[ "$USE_CLOUD" == "true" ]]; then
  echo "☁️  Cua Cloud Sandbox setup complete!"
else
  echo "🖥️  Cua Local VM setup complete!"
fi

echo "📝 Edit $ENV_FILE to update your API keys"
echo "🖥️  Start the playground by running: $DEMO_DIR/start_ui.sh"

# Start the demo automatically
echo
print_info "Starting the Cua Computer-Use Agent UI..."
echo ""

print_success "Cua Computer-Use Agent UI is now running at http://localhost:7860/"
echo
echo "🌐 Open your browser and go to: http://localhost:7860/"
echo
"$DEMO_DIR/start_ui.sh"

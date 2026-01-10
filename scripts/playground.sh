#!/bin/bash

set -e

echo "🚀 Launching Cua Computer-Use Agent UI..."

# Save the original working directory
ORIGINAL_DIR="$(pwd)"

# Directories used by the script
DEMO_DIR="$HOME/.cua-demo"
VENV_DIR="$DEMO_DIR/venv"

# Function to clean up on exit
cleanup() {
  cd ~
  rm -rf "$TMP_DIR" 2>/dev/null || true
}

# Create a temporary directory for our work
TMP_DIR=$(mktemp -d)
cd "$TMP_DIR"
trap cleanup EXIT

# Ask user to choose between local macOS VMs or Cua Cloud Sandbox
echo ""
echo "Choose your Cua setup:"
echo "1) ☁️  Cua Cloud Sandbox (works on any system)"
echo "2) 🖥️  Local macOS VMs (requires Apple Silicon Mac + macOS 15+)"
echo ""
read -p "Enter your choice (1 or 2): " CHOICE

if [[ "$CHOICE" == "1" ]]; then
  # Cua Cloud Sandbox setup
  echo ""
  echo "☁️ Setting up Cua Cloud Sandbox..."
  echo ""
  
  # Check if existing .env.local already has CUA_API_KEY (check current dir and demo dir)
  # Look for .env.local in the original working directory (before cd to temp dir)
  CURRENT_ENV_FILE="$ORIGINAL_DIR/.env.local"
  DEMO_ENV_FILE="$DEMO_DIR/.env.local"
  
  CUA_API_KEY=""
  
  # First check current directory
  if [[ -f "$CURRENT_ENV_FILE" ]] && grep -q "CUA_API_KEY=" "$CURRENT_ENV_FILE"; then
    EXISTING_CUA_KEY=$(grep "CUA_API_KEY=" "$CURRENT_ENV_FILE" | cut -d'=' -f2- | tr -d '"' | tr -d "'" | xargs)
    if [[ -n "$EXISTING_CUA_KEY" && "$EXISTING_CUA_KEY" != "your_cua_api_key_here" && "$EXISTING_CUA_KEY" != "" ]]; then
      CUA_API_KEY="$EXISTING_CUA_KEY"
    fi
  fi
  
  # Then check demo directory if not found in current dir
  if [[ -z "$CUA_API_KEY" ]] && [[ -f "$DEMO_ENV_FILE" ]] && grep -q "CUA_API_KEY=" "$DEMO_ENV_FILE"; then
    EXISTING_CUA_KEY=$(grep "CUA_API_KEY=" "$DEMO_ENV_FILE" | cut -d'=' -f2- | tr -d '"' | tr -d "'" | xargs)
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
      echo "❌ Cua Api Key is required for Cloud Sandbox."
      exit 1
    fi
  fi
  
  USE_CLOUD=true

elif [[ "$CHOICE" == "2" ]]; then
  # Local macOS VM setup
  echo ""
  echo "🖥️ Setting up local macOS VMs..."
  
  # Check for Apple Silicon Mac
  if [[ $(uname -s) != "Darwin" || $(uname -m) != "arm64" ]]; then
    echo "❌ Local macOS VMs require an Apple Silicon Mac (M1/M2/M3/M4)."
    echo "💡 Consider using Cua Cloud Sandbox instead (option 1)."
    exit 1
  fi

  # Check for macOS 15 (Sequoia) or newer
  OSVERSION=$(sw_vers -productVersion)
  if [[ $(echo "$OSVERSION 15.0" | tr " " "\n" | sort -V | head -n 1) != "15.0" ]]; then
    echo "❌ Local macOS VMs require macOS 15 (Sequoia) or newer. You have $OSVERSION."
    echo "💡 Consider using Cua Cloud Sandbox instead (option 1)."
    exit 1
  fi

  USE_CLOUD=false

else
  echo "❌ Invalid choice. Please run the script again and choose 1 or 2."
  exit 1
fi

# Install Lume if not already installed (only for local VMs)
if [[ "$USE_CLOUD" == "false" ]]; then
  if ! command -v lume &> /dev/null; then
    echo "📦 Installing Lume CLI..."
    curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/lume/scripts/install.sh | bash
    
    # Add lume to PATH for this session if it's not already there
    if ! command -v lume &> /dev/null; then
      export PATH="$PATH:$HOME/.local/bin"
    fi
  fi

  # Pull the macOS Cua image if not already present
  if ! lume ls | grep -q "macos-sequoia-cua"; then
    # Check available disk space
    IMAGE_SIZE_GB=30
    AVAILABLE_SPACE_KB=$(df -k $HOME | tail -1 | awk '{print $4}')
    AVAILABLE_SPACE_GB=$(($AVAILABLE_SPACE_KB / 1024 / 1024))
    
    echo "📊 The macOS Cua image will use approximately ${IMAGE_SIZE_GB}GB of disk space."
    echo "   You currently have ${AVAILABLE_SPACE_GB}GB available on your system."
    
    # Prompt for confirmation
    read -p "   Continue? [y]/n: " CONTINUE
    CONTINUE=${CONTINUE:-y}
    
    if [[ $CONTINUE =~ ^[Yy]$ ]]; then
      echo "📥 Pulling macOS Cua image (this may take a while)..."
      lume pull macos-sequoia-cua:latest
    else
      echo "❌ Installation cancelled."
      exit 1
    fi
  fi
fi

# Create a Python virtual environment
echo "🐍 Setting up Python environment..."

# Try different Python commands in order of preference
PYTHON_CMD=""
for cmd in python3.11 python3 python; do
  if command -v $cmd &> /dev/null; then
    # Check this Python version
    PYTHON_VERSION=$($cmd --version 2>&1 | cut -d" " -f2)
    PYTHON_MAJOR=$(echo $PYTHON_VERSION | cut -d. -f1)
    PYTHON_MINOR=$(echo $PYTHON_VERSION | cut -d. -f2)
    
    if [ "$PYTHON_MAJOR" -eq 3 ] && [ "$PYTHON_MINOR" -eq 11 ]; then
      PYTHON_CMD=$cmd
      echo "✅ Found suitable Python: $cmd (version $PYTHON_VERSION)"
      break
    elif [ "$PYTHON_MAJOR" -eq 3 ] && [ "$PYTHON_MINOR" -gt 11 ]; then
      PYTHON_CMD=$cmd
      PYTHON_TOO_NEW=true
      echo "⚠️  Found $cmd (version $PYTHON_VERSION) but only Python 3.11.x is supported."
      break
    else
      echo "⚠️  Found $cmd (version $PYTHON_VERSION) but it's too old, trying next..."
    fi
  fi
done

# If no suitable Python was found, or if Python is too new, offer to exit or continue
if [ -z "$PYTHON_CMD" ] || [ "$PYTHON_TOO_NEW" = true ]; then
  OS_TYPE=$(uname -s)
  if [ "$PYTHON_TOO_NEW" = true ]; then
    echo -e "\n❌ Python version $PYTHON_VERSION detected. Only Python 3.11.x is supported. Newer versions (e.g., 3.12+) are not yet supported."
  else
    if [[ "$OS_TYPE" == "Darwin" ]]; then
      echo -e "\n❌ python3.11 not found. To continue, we recommend running this:\n\n    $ brew install python@3.11\n"
    elif [[ "$OS_TYPE" == "MINGW"* || "$OS_TYPE" == "CYGWIN"* || "$OS_TYPE" == "MSYS"* ]]; then
      echo -e "\n❌ python3.11 not found. Please install Python 3.11 from https://www.python.org/downloads/\n"
    else
      echo -e "\n❌ python3.11 not found. Please install Python 3.11 from your package manager or https://www.python.org/downloads/\n"
    fi
  fi
  while true; do
    echo "Would you like to exit so you can install Python 3.11, or continue anyway? (e = exit, c = continue): "
    read -n 1 -r PYTHON_CONT_CHOICE
    echo
    if [[ "$PYTHON_CONT_CHOICE" =~ ^[Ee]$ ]]; then
      echo "Exiting so you can install Python 3.11."
      exit 1
    elif [[ "$PYTHON_CONT_CHOICE" =~ ^[Cc]$ ]]; then
      echo "⚠️  Continuing without Python 3.11. Some features may not work as expected."
      break
    else
      echo "Please enter 'e' to exit or 'c' to continue."
    fi
  done
fi

# Create a virtual environment
if [ ! -d "$VENV_DIR" ]; then
  $PYTHON_CMD -m venv "$VENV_DIR"
fi

# Activate the virtual environment
source "$VENV_DIR/bin/activate"

# Install required packages
echo "📦 Updating Cua packages..."
pip install -U pip setuptools wheel Cmake
pip install -U cua-computer "cua-agent[all]"

# Create a simple demo script
mkdir -p "$DEMO_DIR"

# Create .env.local file with API keys (only if it doesn't exist)
if [[ ! -f "$DEMO_DIR/.env.local" ]]; then
  cat > "$DEMO_DIR/.env.local" << EOF
# Uncomment and add your API keys here
# OPENAI_API_KEY=your_openai_api_key_here
# ANTHROPIC_API_KEY=your_anthropic_api_key_here
CUA_API_KEY=your_cua_api_key_here
EOF
  echo "📝 Created .env.local file with API key placeholders"
else
  echo "📝 Found existing .env.local file - keeping your current settings"
fi

if [[ "$USE_CLOUD" == "true" ]]; then
  # Add Cua API key to .env.local if not already present
  if ! grep -q "CUA_API_KEY" "$DEMO_DIR/.env.local"; then
    echo "CUA_API_KEY=$CUA_API_KEY" >> "$DEMO_DIR/.env.local"
    echo "🔑 Added CUA_API_KEY to .env.local"
  elif grep -q "CUA_API_KEY=your_cua_api_key_here" "$DEMO_DIR/.env.local"; then
    # Update placeholder with actual key
    sed -i.bak "s/CUA_API_KEY=your_cua_api_key_here/CUA_API_KEY=$CUA_API_KEY/" "$DEMO_DIR/.env.local"
    echo "🔑 Updated CUA_API_KEY in .env.local"
  fi
fi

# Create a convenience script to run the demo
cat > "$DEMO_DIR/start_ui.sh" << EOF
#!/bin/bash
source "$VENV_DIR/bin/activate"
cd "$DEMO_DIR"
python run_demo.py
EOF
chmod +x "$DEMO_DIR/start_ui.sh"

echo "✅ Setup complete!"

if [[ "$USE_CLOUD" == "true" ]]; then
  # Create run_demo.py for cloud sandbox
  cat > "$DEMO_DIR/run_demo.py" << 'EOF'
import asyncio
import os
from pathlib import Path
from dotenv import load_dotenv
from computer import Computer
from agent import ComputerAgent, LLM, AgentLoop, LLMProvider
from agent.ui.gradio.ui_components import create_gradio_ui

# Load environment variables from .env.local
load_dotenv(Path(__file__).parent / ".env.local")

# Check for required API keys
cua_api_key = os.environ.get("CUA_API_KEY", "")
if not cua_api_key:
    print("\n❌ CUA_API_KEY not found in .env.local file.")
    print("Please add your Cua API key to the .env.local file.")
    exit(1)

openai_key = os.environ.get("OPENAI_API_KEY", "")
anthropic_key = os.environ.get("ANTHROPIC_API_KEY", "")

if not openai_key and not anthropic_key:
    print("\n⚠️  No OpenAI or Anthropic API keys found in .env.local.")
    print("Please add at least one API key to use AI agents.")

print("🚀 Starting Cua playground with Cloud Sandbox...")
print("📝 Edit .env.local to update your API keys")

# Launch the Gradio UI and open it in the browser
app = create_gradio_ui()
app.launch(share=False, inbrowser=True)
EOF
else
  # Create run_demo.py for local macOS VMs
  cat > "$DEMO_DIR/run_demo.py" << 'EOF'
import asyncio
import os
from pathlib import Path
from dotenv import load_dotenv
from computer import Computer
from agent import ComputerAgent, LLM, AgentLoop, LLMProvider
from agent.ui.gradio.ui_components import create_gradio_ui

# Load environment variables from .env.local
load_dotenv(Path(__file__).parent / ".env.local")

# Try to load API keys from environment
openai_key = os.environ.get("OPENAI_API_KEY", "")
anthropic_key = os.environ.get("ANTHROPIC_API_KEY", "")

if not openai_key and not anthropic_key:
    print("\n⚠️  No OpenAI or Anthropic API keys found in .env.local.")
    print("Please add at least one API key to use AI agents.")

print("🚀 Starting Cua playground with local macOS VMs...")
print("📝 Edit .env.local to update your API keys")

# Launch the Gradio UI and open it in the browser
app = create_gradio_ui()
app.launch(share=False, inbrowser=True)
EOF
fi

echo "☁️  Cua Cloud Sandbox setup complete!"
echo "📝 Edit $DEMO_DIR/.env.local to update your API keys"
echo "🖥️  Start the playground by running: $DEMO_DIR/start_ui.sh"

# Check if the VM is running (only for local setup)
if [[ "$USE_CLOUD" == "false" ]]; then
  echo "🔍 Checking if the macOS Cua VM is running..."
  VM_RUNNING=$(lume ls | grep "macos-sequoia-cua" | grep "running" || echo "")

  if [ -z "$VM_RUNNING" ]; then
    echo "🚀 Starting the macOS Cua VM in the background..."
    lume run macos-sequoia-cua:latest &
    # Wait a moment for the VM to initialize
    sleep 5
    echo "✅ VM started successfully."
  else
    echo "✅ macOS Cua VM is already running."
  fi
fi

# Ask if the user wants to start the demo now
echo
read -p "Would you like to start the Cua Computer-Use Agent UI now? (y/n) " -n 1 -r
echo
if [[ $REPLY =~ ^[Yy]$ ]]; then
  echo "🚀 Starting the Cua Computer-Use Agent UI..."
  echo ""
  "$DEMO_DIR/start_ui.sh"
fi

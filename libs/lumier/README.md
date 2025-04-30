<div align="center">
<h1>
  <div class="image-wrapper" style="display: inline-block;">
    <picture>
      <source media="(prefers-color-scheme: dark)" alt="logo" height="150" srcset="../../img/logo_white.png" style="display: block; margin: auto;">
      <source media="(prefers-color-scheme: light)" alt="logo" height="150" srcset="../../img/logo_black.png" style="display: block; margin: auto;">
      <img alt="Shows my svg">
    </picture>
  </div>

  [![Swift 6](https://img.shields.io/badge/Swift_6-F54A2A?logo=swift&logoColor=white&labelColor=F54A2A)](#)
  [![macOS](https://img.shields.io/badge/macOS-000000?logo=apple&logoColor=F0F0F0)](#)
  [![Homebrew](https://img.shields.io/badge/Homebrew-FBB040?logo=homebrew&logoColor=fff)](#install)
  [![Discord](https://img.shields.io/badge/Discord-%235865F2.svg?&logo=discord&logoColor=white)](https://discord.com/invite/mVnXXpdE85)
</h1>
</div>

**Lumier** provides a Docker-based interface for the `lume` CLI, allowing you to easily run macOS virtual machines inside a container with VNC access. It creates a secure tunnel to execute lume commands on your host machine while providing a containerized environment for your applications.

## Requirements

Before using Lumier, make sure you have:

1. Install [lume](https://github.com/trycua/cua/blob/main/libs/lume/README.md) on your host machine
2. Docker installed on your host machine
3. `socat` installed for the tunnel (install with Homebrew: `brew install socat`)

## Installation

You can use Lumier directly from its directory or install it to your system:

```bash
# Option 1: Install to your user's bin directory (recommended)
./install.sh

# Option 2: Install to a custom directory
./install.sh --install-dir=/usr/local/bin  # May require sudo

# Option 3: View installation options
./install.sh --help
```

After installation, you can run `lumier` from anywhere in your terminal.

If you get a "command not found" error, make sure the installation directory is in your PATH. The installer will warn you if it isn't and provide instructions to add it.

## Usage

There are two ways to use Lumier: with the provided script or directly with Docker.

### Option 1: Using the Lumier Script

Lumier provides a simple CLI interface to manage VMs in Docker with full Docker compatibility:

```bash
# Show help and available commands
lumier help

# Start the tunnel to connect to lume 
lumier start

# Check if the tunnel is running
lumier status

# Stop the tunnel
lumier stop

# Build the Docker image (optional, happens automatically on first run)
lumier build

# Run a VM with default settings
lumier run -it --rm

# Run a VM with custom settings using Docker's -e flag
lumier run -it --rm \
    --name lumier-vm \
    -p 8006:8006 \
    -v $(pwd)/storage:/storage \
    -v $(pwd)/shared:/data \
    -e VERSION=ghcr.io/trycua/macos-sequoia-cua:latest \
    -e CPU_CORES=4 \
    -e RAM_SIZE=8192
    
# Note:
# The lumier script now automatically detects the real host paths for ./storage and ./shared
# and passes them to the container as HOST_STORAGE_PATH and HOST_DATA_PATH.
# You do NOT need to specify these environment variables manually.
# The VM name is always set from the container name.
```

### Option 2: Using Docker Directly

You can also use Docker commands directly without the lumier utility:

```bash
# 1. Start the tunnel manually
cd libs/lumier
socat TCP-LISTEN:8080,reuseaddr,fork EXEC:"$PWD/src/bin/tunnel.sh" &
TUNNEL_PID=$!

# 2. Build the Docker image
docker build -t lumier:latest .

# 3. Run the container
docker run -it --rm \
    --name lumier-vm \
    -p 8006:8006 \
    -v $(pwd)/storage:/storage \
    -v $(pwd)/shared:/data \
    -e VM_NAME=lumier-vm \
    -e VERSION=ghcr.io/trycua/macos-sequoia-cua:latest \
    -e CPU_CORES=4 \
    -e RAM_SIZE=8192 \
    -e HOST_STORAGE_PATH=$(pwd)/storage \
    -e HOST_DATA_PATH=$(pwd)/shared \
    lumier:latest
    
# 4. Stop the tunnel when you're done
kill $TUNNEL_PID

# Alternatively, find and kill the tunnel process
# First, find the process
lsof -i TCP:8080
# Then kill it by PID
kill <PID>
```

Note that when using Docker directly, you're responsible for:
- Starting and managing the tunnel
- Building the Docker image
- Providing the correct environment variables 

## Available Environment Variables

These variables can be set using Docker's `-e` flag:

- `VM_NAME`: Set the VM name (default: lumier)
- `VERSION`: Set the VM image (default: ghcr.io/trycua/macos-sequoia-vanilla:latest)
- `CPU_CORES`: Set the number of CPU cores (default: 4)
- `RAM_SIZE`: Set the memory size in MB (default: 8192)
- `DISPLAY`: Set the display resolution (default: 1024x768)
- `HOST_DATA_PATH`: Path on the host to share with the VM
- `LUMIER_DEBUG`: Enable debug mode (set to 1)

## Project Structure

The project is organized as follows:

```
lumier/
├── Dockerfile            # Main Docker image definition
├── README.md             # This file
├── lumier                # Main CLI script
├── install.sh            # Installation script
├── src/                  # Source code
│   ├── bin/              # Executable scripts
│   │   ├── entry.sh      # Docker entrypoint
│   │   ├── server.sh     # Tunnel server manager
│   │   └── tunnel.sh     # Tunnel request handler
│   ├── config/           # Configuration
│   │   └── constants.sh  # Shared constants
│   ├── hooks/            # Lifecycle hooks
│   │   └── on-logon.sh   # Run after VM boots
│   └── lib/              # Shared library code
│       ├── utils.sh      # Utility functions
│       └── vm.sh         # VM management functions
└── mount/                # Default shared directory
```

## VNC Access

When a VM is running, you can access it via VNC through:
http://localhost:8006/vnc.html

The password is displayed in the console output when the VM starts.
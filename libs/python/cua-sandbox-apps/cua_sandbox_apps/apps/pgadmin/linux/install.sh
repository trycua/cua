#!/bin/bash
set -e

echo "Installing pgAdmin4..."

# Update package lists
sudo apt-get update

# Install system dependencies
sudo apt-get install -y \
    build-essential \
    libssl-dev \
    libffi-dev \
    python3-dev \
    python3-pip \
    python3-venv \
    postgresql-client

# Create installation directory
INSTALL_DIR="/opt/pgadmin4"
sudo mkdir -p "$INSTALL_DIR"
sudo chown $USER:$USER "$INSTALL_DIR"

# Create Python virtual environment
cd "$INSTALL_DIR"
python3 -m venv venv

# Activate venv and install
bash -c "source venv/bin/activate && pip install --upgrade pip setuptools wheel && pip install pgadmin4"

# Create config directory and file
mkdir -p ~/.pgadmin
cat > ~/.pgadmin/pgadmin4_config.py << 'EOF'
# pgAdmin4 Configuration file - override defaults for user directory

# Use user's home directory for storage
import os
SQLITE_PATH = os.path.expanduser('~/.pgadmin/pgadmin4.db')
STORAGE_DIR = os.path.expanduser('~/.pgadmin')
LOG_FILE = os.path.expanduser('~/.pgadmin/pgadmin4.log')

# Server configuration
CONSOLE_LOG_LEVEL = 10

# Enhanced logging
FILE_LOG_LEVEL = 10

# Desktop mode
DEFAULT_LANGUAGE = 'en'
ENHANCED_COOKIE_PROTECTION = False
COOKIE_DEFAULT = False
COOKIE_SAMESITE = 'Lax'
EOF

echo "pgAdmin4 installation complete!"
echo "pgAdmin has been installed in $INSTALL_DIR"
echo "Config file created at ~/.pgadmin/pgadmin4_config.py"
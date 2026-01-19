#!/bin/bash
# Setup Cua Computer Server on Linux
# Creates a system-level systemd service to run computer server in background

set -e

USER_NAME="docker"
USER_HOME="/home/$USER_NAME"
SCRIPT_DIR="/opt/oem"
PROJECT_DIR="/opt/cua-server"
SERVICE_NAME="cua-computer-server"
LOG_FILE="$SCRIPT_DIR/setup.log"

log() {
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] $1" | tee -a "$LOG_FILE"
}

log "=== Installing Cua Computer Server ==="

# Install Python 3.12 and dependencies
log "Installing Python 3.12 and dependencies..."
sudo apt-get update
sudo apt-get install -y software-properties-common gnome-screenshot curl
sudo add-apt-repository -y ppa:deadsnakes/ppa
sudo apt-get update
sudo apt-get install -y python3.12 python3.12-venv python3.12-tk python3.12-dev build-essential linux-headers-$(uname -r) gcc

# Wait for network to be ready
log "Checking network connectivity..."
NETWORK_RETRIES=30
NETWORK_DELAY=2
NETWORK_READY=false

for i in $(seq 1 $NETWORK_RETRIES); do
    if curl -s --connect-timeout 3 https://astral.sh > /dev/null 2>&1; then
        log "Network is ready"
        NETWORK_READY=true
        break
    fi
    if [ $i -lt $NETWORK_RETRIES ]; then
        log "Network not ready, waiting... (attempt $i/$NETWORK_RETRIES)"
        sleep $NETWORK_DELAY
    fi
done

if [ "$NETWORK_READY" = false ]; then
    log "Network failed to become ready after $NETWORK_RETRIES attempts"
    exit 1
fi

# Install UV package manager as the service user (with retry logic)
log "Installing UV package manager for user $USER_NAME..."
MAX_RETRIES=3
RETRY_DELAY=5
UV_INSTALLED=false

for i in $(seq 1 $MAX_RETRIES); do
    log "UV installation attempt $i of $MAX_RETRIES"
    if sudo -u "$USER_NAME" bash -c "curl -LsSf https://astral.sh/uv/install.sh | sh"; then
        UV_PATH="$USER_HOME/.local/bin/uv"
        if [ -f "$UV_PATH" ]; then
            log "UV installed successfully at $UV_PATH"
            UV_INSTALLED=true
            break
        fi
    fi
    if [ $i -lt $MAX_RETRIES ]; then
        log "UV install attempt $i failed. Retrying in $RETRY_DELAY seconds..."
        sleep $RETRY_DELAY
        RETRY_DELAY=$((RETRY_DELAY * 2))
    fi
done

if [ "$UV_INSTALLED" = false ]; then
    log "UV installation failed after $MAX_RETRIES attempts"
    exit 1
fi

# Add UV to PATH for subsequent commands in this script
export PATH="$USER_HOME/.local/bin:$PATH"

# Create system-wide symlink for UV so it's available as 'uv' command
log "Creating system-wide symlink for UV..."
sudo ln -sf "$USER_HOME/.local/bin/uv" /usr/local/bin/uv
log "UV is now available system-wide"

# Set Python 3.12 as default python3 (after all apt operations are complete)
log "Setting Python 3.12 as default python3..."
sudo update-alternatives --install /usr/bin/python3 python3 /usr/bin/python3.12 1
sudo update-alternatives --set python3 /usr/bin/python3.12

# Create UV project directory
log "Creating UV project at $PROJECT_DIR..."
sudo mkdir -p "$PROJECT_DIR"
sudo chown "$USER_NAME:$USER_NAME" "$PROJECT_DIR"

PROJECT_FILE="$PROJECT_DIR/pyproject.toml"
if [ -f "$PROJECT_FILE" ]; then
    log "Existing UV project detected; skipping creation"
else
    log "Creating UV project..."
    sudo -u "$USER_NAME" uv init --vcs none --no-readme --no-workspace --no-pin-python "$PROJECT_DIR"
    log "UV project created successfully"
fi

# Install packages using UV as the service user
log "Installing cua-computer-server with UV..."
sudo -u "$USER_NAME" uv add --directory "$PROJECT_DIR" cua-computer-server "Pillow>=9.2.0"
log "cua-computer-server installed successfully"

log "Installing playwright & firefox browser..."
sudo -u "$USER_NAME" uv add --directory "$PROJECT_DIR" playwright
sudo -u "$USER_NAME" uv run --directory "$PROJECT_DIR" playwright install firefox
log "playwright installed successfully"

# Open firewall for port 5000 (if ufw is available)
if command -v ufw &> /dev/null; then
    log "Opening firewall for port 5000..."
    sudo ufw allow 5000/tcp || true
    log "Firewall rule added"
fi

# Create start script with auto-restart
START_SCRIPT="$PROJECT_DIR/start-server.sh"
log "Creating start script at $START_SCRIPT..."

cat > "$START_SCRIPT" << 'EOF'
#!/bin/bash
# Cua Computer Server Start Script with auto-restart

PROJECT_DIR="/opt/cua-server"
LOG_FILE="$PROJECT_DIR/server.log"

start_server() {
    echo "$(date '+%Y-%m-%d %H:%M:%S') Updating cua-computer-server with UV..." >> "$LOG_FILE"
    uv add --directory "$PROJECT_DIR" cua-computer-server >> "$LOG_FILE" 2>&1

    echo "$(date '+%Y-%m-%d %H:%M:%S') Starting Cua Computer Server on port 5000..." >> "$LOG_FILE"
    uv run --directory "$PROJECT_DIR" python -m computer_server --port 5000 >> "$LOG_FILE" 2>&1
    return $?
}

while true; do
    start_server
    EXIT_CODE=$?
    echo "$(date '+%Y-%m-%d %H:%M:%S') Server exited with code: $EXIT_CODE. Restarting in 5s..." >> "$LOG_FILE"
    sleep 5
done
EOF

chmod +x "$START_SCRIPT"
log "Start script created"

# Create xhost script for X11 access
log "Creating xhost script..."
sudo tee /etc/X11/Xsession.d/99xauth > /dev/null << 'EOF'
#!/bin/sh
# Grant local X11 access for Cua Computer Server
export DISPLAY=:0
xhost +local: 2>/dev/null || true
EOF
sudo chmod +x /etc/X11/Xsession.d/99xauth
log "X11 access script created"

# Create system-level systemd service
log "Creating systemd system service..."

sudo tee /etc/systemd/system/$SERVICE_NAME.service > /dev/null << EOF
[Unit]
Description=Cua Computer Server
After=graphical.target

[Service]
Type=simple
ExecStart=$START_SCRIPT
Restart=always
RestartSec=5
Environment=PYTHONUNBUFFERED=1
Environment=DISPLAY=:0
Environment=XAUTHORITY=$USER_HOME/.Xauthority
User=$USER_NAME
WorkingDirectory=$PROJECT_DIR

[Install]
WantedBy=graphical.target
EOF

log "Systemd service created at /etc/systemd/system/$SERVICE_NAME.service"

# Ensure proper ownership of project directory
log "Setting ownership of $PROJECT_DIR to $USER_NAME..."
sudo chown -R "$USER_NAME:$USER_NAME" "$PROJECT_DIR"

# Enable and start the service
log "Enabling systemd service..."
sudo systemctl daemon-reload
sudo systemctl enable "$SERVICE_NAME.service"

log "Starting Cua Computer Server service..."
sudo systemctl start "$SERVICE_NAME.service" || true

log "=== Cua Computer Server setup completed ==="
log "Service status: $(sudo systemctl is-active $SERVICE_NAME.service 2>/dev/null || echo 'unknown')"

#!/bin/bash
# Setup CUA Computer Server on Linux
# Creates a system-level systemd service to run computer server in background

set -e

USER_NAME="docker"
USER_HOME="/home/$USER_NAME"
SCRIPT_DIR="/opt/oem"
CUA_DIR="/opt/cua-server"
VENV_DIR="$CUA_DIR/venv"
SERVICE_NAME="cua-computer-server"
LOG_FILE="$SCRIPT_DIR/setup.log"

log() {
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] $1" | tee -a "$LOG_FILE"
}

log "=== Installing CUA Computer Server ==="

# Install Python 3 and venv
log "Installing Python 3 and dependencies..."
sudo apt-get install -y python3 python3-venv python3-pip python3-tk python3-dev gnome-screenshot

# Create CUA directory
log "Creating CUA directory at $CUA_DIR..."
sudo mkdir -p "$CUA_DIR"
sudo chown "$USER_NAME:$USER_NAME" "$CUA_DIR"

# Create virtual environment
if [ -f "$VENV_DIR/bin/python" ]; then
    log "Existing venv detected; skipping creation"
else
    log "Creating Python virtual environment at $VENV_DIR..."
    python3 -m venv "$VENV_DIR"
    log "Virtual environment created successfully"
fi

# Activate and install packages
log "Upgrading pip, setuptools, and wheel..."
"$VENV_DIR/bin/pip" install --upgrade pip setuptools wheel

log "Installing cua-computer-server..."
"$VENV_DIR/bin/pip" install --upgrade cua-computer-server "Pillow>=9.2.0"
log "cua-computer-server installed successfully"

# Open firewall for port 5000 (if ufw is available)
if command -v ufw &> /dev/null; then
    log "Opening firewall for port 5000..."
    sudo ufw allow 5000/tcp || true
    log "Firewall rule added"
fi

# Create start script with auto-restart
START_SCRIPT="$CUA_DIR/start-server.sh"
log "Creating start script at $START_SCRIPT..."

cat > "$START_SCRIPT" << 'EOF'
#!/bin/bash
# CUA Computer Server Start Script with auto-restart

CUA_DIR="/opt/cua-server"
VENV_DIR="$CUA_DIR/venv"
LOG_FILE="$CUA_DIR/server.log"

start_server() {
    echo "$(date '+%Y-%m-%d %H:%M:%S') Updating cua-computer-server..." >> "$LOG_FILE"
    "$VENV_DIR/bin/pip" install --upgrade cua-computer-server >> "$LOG_FILE" 2>&1

    echo "$(date '+%Y-%m-%d %H:%M:%S') Starting CUA Computer Server on port 5000..." >> "$LOG_FILE"
    "$VENV_DIR/bin/python" -m computer_server --port 5000 >> "$LOG_FILE" 2>&1
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
# Grant local X11 access for CUA Computer Server
export DISPLAY=:0
xhost +local: 2>/dev/null || true
EOF
sudo chmod +x /etc/X11/Xsession.d/99xauth
log "X11 access script created"

# Create system-level systemd service
log "Creating systemd system service..."

sudo tee /etc/systemd/system/$SERVICE_NAME.service > /dev/null << EOF
[Unit]
Description=CUA Computer Server
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
WorkingDirectory=$CUA_DIR

[Install]
WantedBy=graphical.target
EOF

log "Systemd service created at /etc/systemd/system/$SERVICE_NAME.service"

# Ensure proper ownership of CUA directory
log "Setting ownership of $CUA_DIR to $USER_NAME..."
sudo chown -R "$USER_NAME:$USER_NAME" "$CUA_DIR"

# Enable and start the service
log "Enabling systemd service..."
sudo systemctl daemon-reload
sudo systemctl enable "$SERVICE_NAME.service"

log "Starting CUA Computer Server service..."
sudo systemctl start "$SERVICE_NAME.service" || true

log "=== CUA Computer Server setup completed ==="
log "Service status: $(sudo systemctl is-active $SERVICE_NAME.service 2>/dev/null || echo 'unknown')"

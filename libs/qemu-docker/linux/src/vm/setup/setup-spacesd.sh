#!/bin/bash
# Setup cua-spacesd on Linux
# Installs the in-sandbox daemon and a system-level systemd service that serves
# gRPC + gRPC-Web on 0.0.0.0:3211 inside the desktop session.
#
# Same artifact as libs/cua-spacesd/packaging/install.sh installs:
#   https://github.com/trycua/cua/releases/download/cua-spacesd-v${VERSION}/cua-spacesd-${ARCH}-unknown-linux-gnu.tar.gz
#
# Token: $CUA_ENV_TOKEN, else /run/cua/env-token, else /etc/cua/env-token.
# Setup writes /etc/cua/env-token (from /opt/oem/env-token when provided,
# otherwise a random one) so the driver never listens unauthenticated.

set -e

USER_NAME="docker"
USER_HOME="/home/$USER_NAME"
SCRIPT_DIR="/opt/oem"
SERVICE_NAME="cua-spacesd"
LOG_FILE="$SCRIPT_DIR/setup.log"
VERSION="${CUA_SPACESD_VERSION:-${CUA_GUESTD_VERSION:-${CUA_ENV_DRIVER_VERSION:-0.1.0}}}"
PORT="${CUA_ENV_PORT:-3211}"

log() {
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] $1" | tee -a "$LOG_FILE"
}

log "=== Installing cua-spacesd $VERSION ==="

sudo apt-get update
sudo apt-get install -y curl ca-certificates x11-xserver-utils

case "$(uname -m)" in
    x86_64|amd64) ARCH=x86_64 ;;
    aarch64|arm64) ARCH=aarch64 ;;
    *) log "Unsupported architecture: $(uname -m)"; exit 1 ;;
esac
URL="https://github.com/trycua/cua/releases/download/cua-spacesd-v${VERSION}/cua-spacesd-${ARCH}-unknown-linux-gnu.tar.gz"

INSTALLED=false
for i in 1 2 3 4 5; do
    if curl -fsSL --connect-timeout 10 "$URL" | sudo tar -xz -C /usr/local/bin cua-spacesd; then
        INSTALLED=true
        break
    fi
    log "Download attempt $i failed; retrying in $((i * 5))s..."
    sleep $((i * 5))
done
if [ "$INSTALLED" = false ]; then
    log "Failed to download $URL"
    exit 1
fi
sudo chmod 0755 /usr/local/bin/cua-spacesd
sudo ln -sfn cua-spacesd /usr/local/bin/cua-guestd  # older names, one release
sudo ln -sfn cua-spacesd /usr/local/bin/cua-env-driver
log "cua-spacesd installed at /usr/local/bin/cua-spacesd"

# Persistent token
sudo install -d -m 0755 /etc/cua
if [ -s "$SCRIPT_DIR/env-token" ]; then
    sudo install -m 0640 -o root -g "$USER_NAME" "$SCRIPT_DIR/env-token" /etc/cua/env-token
elif [ ! -s /etc/cua/env-token ]; then
    head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' | sudo tee /etc/cua/env-token >/dev/null
    sudo chown root:"$USER_NAME" /etc/cua/env-token
    sudo chmod 0640 /etc/cua/env-token
fi
log "Token stored at /etc/cua/env-token"

if command -v ufw &> /dev/null; then
    log "Opening firewall for port $PORT..."
    sudo ufw allow "$PORT/tcp" || true
fi

START_SCRIPT="/usr/local/bin/start-cua-spacesd"
sudo tee "$START_SCRIPT" > /dev/null << 'EOF2'
#!/bin/bash
# Start cua-spacesd; the token reaches it through the environment, never argv.
set -euo pipefail
token="${CUA_ENV_TOKEN:-}"
for f in /run/cua/env-token /etc/cua/env-token; do
    [ -n "$token" ] && break
    [ -r "$f" ] && token="$(tr -d '\r\n' <"$f")"
done
if [ -z "$token" ]; then
    echo "no cua-spacesd token (CUA_ENV_TOKEN, /run/cua/env-token, /etc/cua/env-token); refusing to start" >&2
    exit 1
fi
export CUA_ENV_TOKEN="$token"
exec /usr/local/bin/cua-spacesd --listen "0.0.0.0:${CUA_ENV_PORT:-3211}"
EOF2
sudo chmod 0755 "$START_SCRIPT"

# Grant local X11 access to the driver (runs as the desktop user)
sudo tee /etc/X11/Xsession.d/99xauth > /dev/null << 'EOF2'
#!/bin/sh
export DISPLAY=:0
xhost +local: 2>/dev/null || true
EOF2
sudo chmod +x /etc/X11/Xsession.d/99xauth

sudo tee /etc/systemd/system/$SERVICE_NAME.service > /dev/null << EOF2
[Unit]
Description=cua-spacesd (in-sandbox daemon, :$PORT)
After=graphical.target

[Service]
Type=simple
ExecStart=$START_SCRIPT
Restart=always
RestartSec=5
Environment=CUA_ENV_PORT=$PORT
Environment=DISPLAY=:0
Environment=XAUTHORITY=$USER_HOME/.Xauthority
User=$USER_NAME

[Install]
WantedBy=graphical.target
Alias=cua-env-driver.service
Alias=cua-guestd.service
EOF2

sudo systemctl daemon-reload
sudo systemctl enable "$SERVICE_NAME.service"
sudo systemctl start "$SERVICE_NAME.service" || true

log "=== cua-spacesd setup completed ==="
log "Service status: $(sudo systemctl is-active $SERVICE_NAME.service 2>/dev/null || echo 'unknown')"

#!/usr/bin/env bash
set -Eeuo pipefail

info () { printf "%b%s%b" "\E[1;34m❯ \E[1;36m" "${1:-}" "\E[0m\n"; }
error () { printf "%b%s%b" "\E[1;31m❯ " "ERROR: ${1:-}" "\E[0m\n" >&2; }
warn () { printf "%b%s%b" "\E[1;31m❯ " "Warning: ${1:-}" "\E[0m\n" >&2; }

# Start the original docker-android entrypoint in background
# This handles emulator startup, VNC, noVNC, etc.
info "Starting \"${EMULATOR_DEVICE}\" emulator..."
/home/androidusr/docker-android/mixins/scripts/run.sh &

# Wait for ADB device to appear and boot to complete
info "Waiting for emulator to be ready..."
counter=0
timeout=300
while [ $counter -lt $timeout ]; do
    if adb devices 2>/dev/null | grep -q "emulator"; then
        # Check if boot is complete
        boot_completed=$(adb shell getprop sys.boot_completed 2>&1 | tr -d '\r\n' | grep -o "1" || echo "0")
        if [ "$boot_completed" = "1" ]; then
            info "✓ Emulator \"${EMULATOR_DEVICE}\" is ready!"
            break
        fi
    fi
    sleep 2
    counter=$((counter + 2))

    # Show progress every 10 seconds
    if [ $((counter % 10)) -eq 0 ]; then
        info "  Still waiting... ($counter/$timeout seconds)"
    fi
done

if [ $counter -ge $timeout ]; then
    error "✗ Emulator \"${EMULATOR_DEVICE}\" failed to start within $timeout seconds"
    exit 1
fi

sleep 5

if adb shell pm list packages | grep -q "com.example.cua.wallpaper"; then
    info "✓ Wallpaper Manager already installed"
else
    info "Installing wallpaper-manager.apk..."
    adb install -r /opt/apks/wallpaper-manager.apk
    if [ $? -eq 0 ]; then
        info "✓ Wallpaper Manager installed successfully"
    else
        warn "✗ Failed to install Wallpaper Manager APK"
    fi
fi

info "Starting Computer Server..."
source /opt/venv/bin/activate
DISPLAY= python -m computer_server --host 0.0.0.0 --port 8000 --log-level info

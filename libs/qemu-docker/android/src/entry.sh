#!/usr/bin/env bash
set -Eeuo pipefail

# Ensure UV is in PATH
export PATH="/usr/local/bin:$PATH"

info () { printf "%b%s%b" "\E[1;34m❯ \E[1;36m" "${1:-}" "\E[0m\n"; }
error () { printf "%b%s%b" "\E[1;31m❯ " "ERROR: ${1:-}" "\E[0m\n" >&2; }
warn () { printf "%b%s%b" "\E[1;31m❯ " "Warning: ${1:-}" "\E[0m\n" >&2; }

# Kill any existing ADB server and emulator processes
info "Cleaning up any existing ADB/emulator processes..."
adb kill-server 2>/dev/null || true
pkill -f "qemu-system" 2>/dev/null || true
sleep 2

# Start ADB server
info "Starting ADB server..."
adb start-server

# Start the original docker-android entrypoint in background
# This handles emulator startup, VNC, noVNC, etc.
info "Starting \"${EMULATOR_DEVICE}\" emulator..."
/home/androidusr/docker-android/mixins/scripts/run.sh &

# Wait for ADB device to appear and boot to complete
info "Waiting for emulator to be ready..."
counter=0
timeout=600
while [ $counter -lt $timeout ]; do
    # Check if emulator device is listed
    emulator_found=$(adb devices 2>/dev/null | grep -c "emulator" 2>/dev/null || echo "0")
    # Strip whitespace and ensure it's a valid integer
    emulator_found=$(echo "$emulator_found" | tr -d '\n\r' | tr -d ' ')
    # Default to 0 if empty or not a number
    if ! [[ "$emulator_found" =~ ^[0-9]+$ ]]; then
        emulator_found=0
    fi
    if [ "$emulator_found" -gt 0 ]; then
        # Check if boot is complete
        boot_completed=$(adb shell getprop sys.boot_completed 2>&1 | tr -d '\r\n' | grep -o "1" || echo "0")
        if [ "$boot_completed" = "1" ]; then
            info "✓ Emulator \"${EMULATOR_DEVICE}\" is ready!"
            break
        else
            # Show what we're waiting for
            if [ $((counter % 20)) -eq 0 ]; then
                info "  Emulator detected, waiting for boot to complete... ($counter/$timeout seconds)"
            fi
        fi
    else
        # Show that we're still waiting for device
        if [ $((counter % 20)) -eq 0 ]; then
            info "  Waiting for emulator device... ($counter/$timeout seconds)"
        fi
    fi
    sleep 2
    counter=$((counter + 2))
done

if [ $counter -ge $timeout ]; then
    error "✗ Emulator \"${EMULATOR_DEVICE}\" failed to start within $timeout seconds"
    info "ADB devices:"
    adb devices
    info "Emulator processes:"
    ps aux | grep qemu || echo "No qemu processes found"
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
DISPLAY= uv run --directory /opt/cua-server python -m computer_server --host 0.0.0.0 --port 8000 --log-level info

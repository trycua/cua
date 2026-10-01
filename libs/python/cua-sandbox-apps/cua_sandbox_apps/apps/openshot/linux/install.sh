#!/bin/bash
set -e

echo "Installing OpenShot Video Editor..."

# Update package list
sudo apt-get update -qq

# Install OpenShot + dummy audio so "no channels" dialog doesn't appear
sudo apt-get install -y openshot-qt pulseaudio

# Start PulseAudio with a null sink (no real hardware needed)
pulseaudio --start --exit-idle-time=-1 2>/dev/null || true
pactl load-module module-null-sink sink_name=dummy 2>/dev/null || true

echo "OpenShot installation completed successfully!"
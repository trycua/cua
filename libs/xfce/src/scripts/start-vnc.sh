#!/bin/bash
set -e

# Clean up any existing VNC lock files
rm -rf /tmp/.X1-lock /tmp/.X11-unix/X1

# Start VNC server without password authentication
vncserver :1 \
    -geometry ${VNC_RESOLUTION:-1920x1080} \
    -depth ${VNC_COL_DEPTH:-24} \
    -rfbport ${VNC_PORT:-5901} \
    -localhost no \
    -SecurityTypes None \
    -AlwaysShared \
    -AcceptPointerEvents \
    -AcceptKeyEvents \
    -AcceptCutText \
    -SendCutText \
    -xstartup /usr/local/bin/xstartup.sh \
    --I-KNOW-THIS-IS-INSECURE

# Keep the process running
tail -f /home/cua/.vnc/*.log

#!/bin/bash
set -e

# Clean up any existing VNC lock files
rm -rf /tmp/.X1-lock /tmp/.X11-unix/X1

# Set up VNC authentication if VNC_PW is provided
if [ -n "$VNC_PW" ]; then
    mkdir -p /home/cua/.vnc
    echo "$VNC_PW" | vncpasswd -f > /home/cua/.vnc/passwd
    chmod 600 /home/cua/.vnc/passwd
    SECURITY_ARGS="-SecurityTypes VncAuth -rfbauth /home/cua/.vnc/passwd"
else
    SECURITY_ARGS="-SecurityTypes None --I-KNOW-THIS-IS-INSECURE"
fi

# Start VNC server
vncserver :1 \
    -geometry ${VNC_RESOLUTION:-1920x1080} \
    -depth ${VNC_COL_DEPTH:-24} \
    -rfbport ${VNC_PORT:-5901} \
    -localhost no \
    $SECURITY_ARGS \
    -AlwaysShared \
    -AcceptPointerEvents \
    -AcceptKeyEvents \
    -AcceptCutText \
    -SendCutText \
    -xstartup /usr/local/bin/xstartup.sh

# Keep the process running
tail -f /home/cua/.vnc/*.log

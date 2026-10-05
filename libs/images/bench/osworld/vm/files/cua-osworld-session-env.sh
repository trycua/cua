#!/usr/bin/env bash
# Write /run/cua-desktop/desktop.env for cua-spacesd from the OSWorld image's
# own desktop: GDM auto-login of `user` on Xorg :0 (VM), or Xvnc :0 started
# by supervisord (container). start-spacesd.sh sources it.
set -euo pipefail
RUNTIME_DIR="${CUA_DESKTOP_RUNTIME_DIR:-/run/cua-desktop}"
DISPLAY_NUM="${CUA_DISPLAY:-:0}"
UID_NUM="$(id -u)"
for _ in $(seq 1 180); do
    [ -S "/tmp/.X11-unix/X${DISPLAY_NUM#:}" ] && break
    sleep 1
done
xauth=""
for f in "/run/user/$UID_NUM/gdm/Xauthority" "$HOME/.Xauthority"; do
    [ -r "$f" ] && { xauth="$f"; break; }
done
mkdir -p "$RUNTIME_DIR"
{
    echo "DISPLAY=$DISPLAY_NUM"
    [ -n "$xauth" ] && echo "XAUTHORITY=$xauth"
    if [ -S "/run/user/$UID_NUM/bus" ]; then
        echo "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$UID_NUM/bus"
        echo "XDG_RUNTIME_DIR=/run/user/$UID_NUM"
    fi
} >"$RUNTIME_DIR/desktop.env.tmp"
mv "$RUNTIME_DIR/desktop.env.tmp" "$RUNTIME_DIR/desktop.env"
echo "desktop env: $(tr '\n' ' ' <"$RUNTIME_DIR/desktop.env")"

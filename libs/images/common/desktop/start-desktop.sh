#!/usr/bin/env bash
# Start the virtual X display and the XFCE session for the desktop user.
#
# One supervised program owns the whole desktop: the X server (Xvfb, or
# TigerVNC's Xvnc when the image also serves RFB), a session D-Bus daemon listens on
# a FIXED address ($XDG_RUNTIME_DIR/bus) so that out-of-session processes
# (cua-spacesd, test harnesses, `docker exec`) can reach the same AT-SPI
# registry the apps register with, and startxfce4 runs the session. If any of
# the three exits, the script exits non-zero so supervisord/systemd restarts
# the lot instead of leaving a half-alive desktop.
#
# Shared by the linux image (Xvfb; cua-spacesd captures and streams the
# display) and the plain ubuntu-xfce-vnc image (Xvnc, RFB only). Nothing here
# knows about cua daemons.
#
# Environment (all optional):
#   CUA_X_SERVER       xvfb or xvnc (default xvnc)
#   CUA_DISPLAY        X display, default :1
#   CUA_RESOLUTION     WxH, default 1280x800
#   CUA_VNC_PORT       RFB port (xvnc), default 5901
#   CUA_VNC_LOCALHOST  1 = bind RFB to loopback only (xvnc, default 0)
#   VNC_PASSWORD       if set, require VncAuth with this password (xvnc)
set -euo pipefail

DISPLAY_NUM="${CUA_DISPLAY:-:1}"
N="${DISPLAY_NUM#:}"
GEOMETRY="${CUA_RESOLUTION:-1280x800}"
VNC_PORT="${CUA_VNC_PORT:-5901}"
VNC_LOCALHOST="${CUA_VNC_LOCALHOST:-0}"
X_SERVER="${CUA_X_SERVER:-xvnc}"

log() { echo "[$(date -Iseconds)] start-desktop: $*"; }

export HOME="${HOME:-/home/$(id -un)}"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/cua-desktop}"
if [ ! -d "$XDG_RUNTIME_DIR" ]; then
    log "XDG_RUNTIME_DIR $XDG_RUNTIME_DIR missing; the entrypoint/tmpfiles should create it"
    exit 1
fi

rm -f "/tmp/.X${N}-lock" "/tmp/.X11-unix/X${N}"

SECURITY_ARGS=(-SecurityTypes None)
if [ "$X_SERVER" = xvnc ] && [ -n "${VNC_PASSWORD:-}" ]; then
    mkdir -p "$HOME/.vnc"
    printf '%s\n' "$VNC_PASSWORD" | vncpasswd -f >"$HOME/.vnc/passwd"
    chmod 600 "$HOME/.vnc/passwd"
    SECURITY_ARGS=(-SecurityTypes VncAuth -PasswordFile "$HOME/.vnc/passwd")
fi
unset VNC_PASSWORD

PIDS=()
cleanup() {
    for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null || true; done
    wait 2>/dev/null || true
}
trap cleanup EXIT TERM INT

case "$X_SERVER" in
xvfb)
    # Xvfb: XTEST (cua-driver input), MIT-SHM and DAMAGE (cua-spacesd
    # capture) and RANDR are built in; no network listener at all.
    log "Xvfb ${DISPLAY_NUM} ${GEOMETRY}"
    Xvfb "${DISPLAY_NUM}" -screen 0 "${GEOMETRY}x24" -dpi 96 \
        -nolisten tcp +extension RANDR +extension GLX -noreset &
    ;;
xvnc)
    log "Xvnc ${DISPLAY_NUM} ${GEOMETRY} rfbport=${VNC_PORT} localhost=${VNC_LOCALHOST}"
    Xvnc "${DISPLAY_NUM}" \
        -geometry "$GEOMETRY" -depth 24 \
        -rfbport "$VNC_PORT" -localhost="$VNC_LOCALHOST" \
        "${SECURITY_ARGS[@]}" \
        -AlwaysShared -AcceptSetDesktopSize=1 \
        -nolisten tcp -desktop "cua" &
    ;;
*)
    log "unknown CUA_X_SERVER=$X_SERVER (xvfb or xvnc)"
    exit 1
    ;;
esac
PIDS+=($!)

for _ in $(seq 1 100); do
    [ -S "/tmp/.X11-unix/X${N}" ] && break
    sleep 0.1
done
[ -S "/tmp/.X11-unix/X${N}" ] || { log "$X_SERVER did not create its socket"; exit 1; }
export DISPLAY="$DISPLAY_NUM"

BUS="$XDG_RUNTIME_DIR/bus"
rm -f "$BUS"
dbus-daemon --session --address="unix:path=$BUS" --nofork --nopidfile --syslog-only &
PIDS+=($!)
for _ in $(seq 1 50); do
    [ -S "$BUS" ] && break
    sleep 0.1
done
export DBUS_SESSION_BUS_ADDRESS="unix:path=$BUS"

# Accessibility on for every toolkit so AT-SPI sees the whole tree.
export GTK_MODULES="${GTK_MODULES:+$GTK_MODULES:}gail:atk-bridge"
export GNOME_ACCESSIBILITY=1 QT_ACCESSIBILITY=1 QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1
export MOZ_ENABLE_ACCESSIBILITY=1 ACCESSIBILITY_ENABLED=1
unset NO_AT_BRIDGE SESSION_MANAGER

# gVisor (runsc) doesn't give Firefox's per-process sandboxes what they need
# (its content/RDD/GPU children die with "VideoBridgeParent ... AbnormalShutdown"
# and no window ever maps). The container boundary is the sandbox there, so
# turn Firefox's inner ones off only when running under gVisor.
SANDBOX_ENV=""
if dmesg 2>/dev/null | head -1 | grep -q gVisor; then
    for v in MOZ_DISABLE_CONTENT_SANDBOX MOZ_DISABLE_GMP_SANDBOX MOZ_DISABLE_RDD_SANDBOX \
        MOZ_DISABLE_SOCKET_PROCESS_SANDBOX MOZ_DISABLE_UTILITY_SANDBOX; do
        export "$v=1"; SANDBOX_ENV="$SANDBOX_ENV$v=1
"
    done
    log "gVisor detected: Firefox inner sandboxes disabled"
fi

# Chromium keeps its own sandbox wherever the runtime allows it: gVisor gives
# it namespaces and seccomp; a plain container without unprivileged user
# namespaces (docker's default seccomp profile) cannot, and there the
# container boundary is the sandbox, so cua-driver launches Chromium with
# --no-sandbox (CUA_DRIVER_BROWSER_NO_SANDBOX).
if [ -z "${CUA_DRIVER_BROWSER_NO_SANDBOX:-}" ] && ! dmesg 2>/dev/null | head -1 | grep -q gVisor \
    && ! unshare --user --map-root-user true 2>/dev/null; then
    SANDBOX_ENV="${SANDBOX_ENV}CUA_DRIVER_BROWSER_NO_SANDBOX=1
"
    log "no unprivileged user namespaces: Chromium runs without its own sandbox"
fi

# Persist the session environment so `docker exec` / helpers can source it.
cat >"$XDG_RUNTIME_DIR/desktop.env" <<EOF
DISPLAY=$DISPLAY
DBUS_SESSION_BUS_ADDRESS=$DBUS_SESSION_BUS_ADDRESS
XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR
GTK_MODULES=$GTK_MODULES
GNOME_ACCESSIBILITY=1
EOF
printf '%s' "$SANDBOX_ENV" >>"$XDG_RUNTIME_DIR/desktop.env"

log "starting XFCE session"
startxfce4 >"$XDG_RUNTIME_DIR/xfce-session.log" 2>&1 &
PIDS+=($!)

# Flip the AT-SPI "enabled" switch once the a11y bus is up. Best-effort: the
# toolkit bridges above already load without it.
(
    for _ in $(seq 1 60); do
        if busctl --user --no-pager status org.a11y.Bus >/dev/null 2>&1 \
            || dbus-send --session --print-reply --dest=org.a11y.Bus /org/a11y/bus org.a11y.Bus.GetAddress >/dev/null 2>&1; then
            dbus-send --session --type=method_call --dest=org.a11y.Bus /org/a11y/bus \
                org.freedesktop.DBus.Properties.Set string:org.a11y.Status string:IsEnabled variant:boolean:true \
                >/dev/null 2>&1 || true
            log "AT-SPI bus enabled"
            break
        fi
        sleep 1
    done
) &

log "desktop up on ${DISPLAY}"
# Exit as soon as any core process dies.
wait -n "${PIDS[@]}"
rc=$?
log "a desktop process exited (rc=$rc); tearing down"
exit 1

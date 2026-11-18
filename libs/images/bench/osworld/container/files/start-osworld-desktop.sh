#!/usr/bin/env bash
# The OSWorld desktop without systemd or GDM: the guest's own Xorg with the
# dummy video driver on :0 (1920x1080, as the VM), a session D-Bus at the
# address the OSWorld server expects (/run/user/<uid>/bus), and the `ubuntu`
# GNOME session in builtin mode (no systemd --user). Runs as `user`; if any
# part exits, the script exits so supervisord restarts the whole desktop.
set -euo pipefail
DISPLAY_NUM="${CUA_DISPLAY:-:0}"
N="${DISPLAY_NUM#:}"
export HOME="/home/$(id -un)"
export XDG_RUNTIME_DIR="/run/user/$(id -u)"
export XAUTHORITY="$HOME/.Xauthority"
log() { echo "[$(date -Iseconds)] osworld-desktop: $*"; }
rm -f "/tmp/.X${N}-lock" "/tmp/.X11-unix/X${N}"
PIDS=()
cleanup() { for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null || true; done; wait 2>/dev/null || true; }
trap cleanup EXIT TERM INT

# A fresh cookie per start; cua-spacesd and the OSWorld server read the file.
rm -f "$XAUTHORITY"
xauth -q -f "$XAUTHORITY" add "$DISPLAY_NUM" . "$(mcookie)"

log "Xorg $DISPLAY_NUM (dummy driver)"
/usr/lib/xorg/Xorg "$DISPLAY_NUM" -config /etc/X11/cua-dummy.conf -auth "$XAUTHORITY" \
    -noreset -nolisten tcp -novtswitch -keeptty -logfile "/tmp/Xorg.$N.log" &
PIDS+=($!)
for _ in $(seq 1 100); do [ -S "/tmp/.X11-unix/X$N" ] && break; sleep 0.1; done
[ -S "/tmp/.X11-unix/X$N" ] || { log "Xorg did not start"; tail -20 "/tmp/Xorg.$N.log" || true; exit 1; }
export DISPLAY="$DISPLAY_NUM"
# Never blank on the X side (the config sets the same; xset covers a server
# started with defaults).
xset s off s noblank -dpms 2>/dev/null || true

BUS="$XDG_RUNTIME_DIR/bus"
rm -f "$BUS"
dbus-daemon --session --address="unix:path=$BUS" --nofork --nopidfile --syslog-only &
PIDS+=($!)
for _ in $(seq 1 50); do [ -S "$BUS" ] && break; sleep 0.1; done
export DBUS_SESSION_BUS_ADDRESS="unix:path=$BUS"
export XDG_SESSION_TYPE=x11 XDG_CURRENT_DESKTOP=ubuntu:GNOME GNOME_SHELL_SESSION_MODE=ubuntu \
    XDG_SESSION_DESKTOP=ubuntu DESKTOP_SESSION=ubuntu \
    XDG_DATA_DIRS=/usr/share/ubuntu:/usr/local/share:/usr/share:/var/lib/snapd/desktop \
    XDG_CONFIG_DIRS=/etc/xdg/xdg-ubuntu:/etc/xdg GTK_MODULES=gail:atk-bridge
# No GPU in a container: Mesa's software renderer. Under qemu-user
# emulation (an amd64 image on an arm64 host) llvmpipe's JIT renders
# nothing, so the interpreter (softpipe) is used there.
export LIBGL_ALWAYS_SOFTWARE=1
if grep -qm1 "^CPU implementer" /proc/cpuinfo 2>/dev/null && [ "$(uname -m)" = x86_64 ]; then
    export GALLIUM_DRIVER="${CUA_GALLIUM_DRIVER:-softpipe}"
    log "emulated x86_64 on an arm host: GALLIUM_DRIVER=$GALLIUM_DRIVER"
fi

# The ubuntu session without gsd-power as a required component: it exits
# without logind (no systemd in a container), which makes gnome-session show
# its failure screen. Everything else is the upstream session file.
SESSIONS="$XDG_RUNTIME_DIR/cua-gnome/gnome-session/sessions"
mkdir -p "$SESSIONS"
sed 's/org\.gnome\.SettingsDaemon\.Power;//' /usr/share/gnome-session/sessions/ubuntu.session >"$SESSIONS/ubuntu.session"
# gsd-power is not started at all (its autostart entry is hidden): without
# logind and UPower it asks the shell to lock, locking needs GDM, and the
# shell keeps its black shield up until input arrives (the "Screen Lock
# disabled" notice). A container has no power management; the upstream
# idle-delay=0 keeps idle blanking off.
mkdir -p "$XDG_RUNTIME_DIR/cua-gnome/autostart"
printf '[Desktop Entry]\nType=Application\nName=Power (disabled in the container)\nExec=/bin/true\nHidden=true\n' \
    >"$XDG_RUNTIME_DIR/cua-gnome/autostart/org.gnome.SettingsDaemon.Power.desktop"
export XDG_CONFIG_DIRS="$XDG_RUNTIME_DIR/cua-gnome:$XDG_CONFIG_DIRS"

log "gnome-session (builtin)"
gnome-session --builtin --session=ubuntu &
PIDS+=($!)
wait -n "${PIDS[@]}"
log "a desktop process exited; restarting the desktop"
exit 1

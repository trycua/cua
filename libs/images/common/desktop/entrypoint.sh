#!/usr/bin/env bash
# Container entrypoint (docker / gVisor rootfs variant). The VM variant uses
# systemd units instead and never runs this.
#
# Prepares runtime dirs the desktop needs (there is no logind/tmpfiles in a
# container), optionally installs SSH credentials, then execs supervisord as
# PID 1.
set -euo pipefail
DESKTOP_USER="${CUA_DESKTOP_USER:-cua}"
install -d -m 0700 -o "$DESKTOP_USER" -g "$DESKTOP_USER" /run/cua-desktop
install -d -m 1777 /tmp/.X11-unix /tmp/.ICE-unix
install -d -m 0755 /var/log/supervisor
# The system bus runs as messagebus and creates its socket here.
if id messagebus >/dev/null 2>&1; then
    install -d -m 0755 -o messagebus -g messagebus /run/dbus
else
    install -d -m 0755 /run/dbus
fi
# WirePlumber's logind monitor watches these and exits when they are missing,
# which takes the audio stack down; a runtime that mounts an empty /run
# (Modal's VM runtime) has none of the image's.
install -d -m 0755 /run/systemd/seats /run/systemd/sessions /run/systemd/users
rm -f /run/dbus/pid
# Each container is its own machine: the image ships an empty
# /etc/machine-id (dbus needs one), so make this container's now. Kept when a
# runtime or an earlier start already set one.
if [ -w /etc/machine-id ] && [ ! -s /etc/machine-id ]; then
    tr -d '-' </proc/sys/kernel/random/uuid >/etc/machine-id
fi

# Same knobs as the plain ubuntu-server image, so any image can be reached over
# SSH when the harness asks for it.
if [ -n "${SSH_AUTHORIZED_KEYS:-}" ]; then
    install -d -m 0700 -o "$DESKTOP_USER" -g "$DESKTOP_USER" "/home/$DESKTOP_USER/.ssh"
    printf '%s\n' "$SSH_AUTHORIZED_KEYS" >"/home/$DESKTOP_USER/.ssh/authorized_keys"
    chown "$DESKTOP_USER:$DESKTOP_USER" "/home/$DESKTOP_USER/.ssh/authorized_keys"
    chmod 600 "/home/$DESKTOP_USER/.ssh/authorized_keys"
fi

# Hook for image-specific preparation (e.g. spacesd token).
for hook in /opt/cua/entrypoint.d/*.sh; do
    [ -e "$hook" ] || continue
    # shellcheck disable=SC1090
    . "$hook"
done

# supervisord starts its programs only after its first one-second poll comes
# back empty, so every boot waited a second for nothing. One connection to
# its control socket ends that first poll as soon as the socket exists.
(
    for _ in $(seq 1 100); do
        [ -S /run/supervisor.sock ] && break
        sleep 0.01
    done
    python3 -c 'import socket; s = socket.socket(socket.AF_UNIX); s.connect("/run/supervisor.sock"); s.close()' \
        2>/dev/null || true
) &

exec /usr/bin/supervisord -n -c /etc/supervisor/supervisord.conf

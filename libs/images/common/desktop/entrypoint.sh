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

exec /usr/bin/supervisord -n -c /etc/supervisor/supervisord.conf

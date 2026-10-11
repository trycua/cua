#!/usr/bin/env bash
# OSWorld container entrypoint: runtime dirs systemd/logind would create in
# the VM, the spacesd token, then supervisord as PID 1.
set -euo pipefail
U="${CUA_DESKTOP_USER:-user}"
UID_NUM="$(id -u "$U")"
install -d -m 0700 -o "$U" -g "$U" "/run/user/$UID_NUM" /run/cua-desktop
install -d -m 1777 /tmp /tmp/.X11-unix /tmp/.ICE-unix
install -d -m 0755 /run/dbus /var/log/supervisor
rm -f /run/dbus/pid /tmp/.X0-lock /tmp/.X11-unix/X0
/opt/cua/bin/ensure-env-token.sh
# The token now lives in /run/cua/env-token; keep it out of the environment
# every supervisord program (the desktop and its apps) would inherit.
unset CUA_ENV_TOKEN CUA_SPACESD_TOKEN
exec /usr/local/bin/supervisord -n -c /etc/supervisor/supervisord.conf

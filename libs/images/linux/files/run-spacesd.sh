#!/usr/bin/env bash
# Start cua-spacesd (start-spacesd.sh) as the desktop user with exactly the
# capabilities it needs to run commands and file operations as another guest
# user: CAP_SETUID, CAP_SETGID and CAP_KILL (to signal what it started as
# another user). Runs as root under supervisord (the container variant; the
# VM variant gets the same through AmbientCapabilities= in
# cua-spacesd.service).
#
# No sudo is involved, so it works where sudo cannot: under gVisor and any
# runtime that sets no-new-privileges or mounts nosuid. The agent stays the
# desktop user (uid 1000): its default user for commands and files does not
# change, it never becomes root, and it empties the inheritable and ambient
# sets at start, so only the agent process itself holds the three
# capabilities; everything it starts is an ordinary process of its user.
# The capabilities must be in the container's bounding set (the Docker, runc
# and gVisor defaults include them). When they are not, or setpriv is
# missing, the agent starts as the plain desktop user and the client falls
# back to sudo, as before.
set -euo pipefail
USER_NAME="${CUA_DESKTOP_USER:-cua}"
# setpriv capability list; CUA_SPACESD_CAPS=none starts the agent without.
CAPS="${CUA_SPACESD_CAPS:-+setuid,+setgid,+kill}"
START="${CUA_SPACESD_START:-/opt/cua/bin/start-spacesd.sh}"
export HOME="/home/$USER_NAME" USER="$USER_NAME" LOGNAME="$USER_NAME"
AS_USER=(setpriv --reuid "$USER_NAME" --regid "$USER_NAME" --init-groups)

if [ "$(id -u)" != 0 ]; then
    exec "$START"
fi
if [ "$CAPS" != none ] && command -v setpriv >/dev/null 2>&1; then
    WITH_CAPS=("${AS_USER[@]}" --inh-caps "$CAPS" --ambient-caps "$CAPS")
    if "${WITH_CAPS[@]}" -- true 2>/dev/null; then
        exec "${WITH_CAPS[@]}" -- "$START"
    fi
    echo "[$(date -Iseconds)] cannot grant $CAPS to the agent (not in the bounding set?); starting it without; commands as other users will use sudo"
fi
if command -v setpriv >/dev/null 2>&1; then
    exec "${AS_USER[@]}" -- "$START"
fi
exec runuser -u "$USER_NAME" -- "$START"

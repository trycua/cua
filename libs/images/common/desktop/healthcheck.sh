#!/usr/bin/env bash
# Liveness: the X display answers (and, with Xvnc, the RFB port is open).
# Daemon-agnostic on purpose -- images add their own checks via
# /opt/cua/healthcheck.d/*.sh.
set -euo pipefail
DISPLAY="${CUA_DISPLAY:-:1}" xdpyinfo >/dev/null 2>&1
if [ "${CUA_X_SERVER:-xvnc}" = xvnc ]; then
    # Listening check via ss rather than a TCP connect: connects would spam
    # the Xvnc log every probe interval.
    ss -Hltn "sport = :${CUA_VNC_PORT:-5901}" | grep -q .
fi
for check in /opt/cua/healthcheck.d/*.sh; do
    [ -e "$check" ] || continue
    bash "$check"
done

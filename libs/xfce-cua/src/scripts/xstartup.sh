#!/bin/bash
set -euo pipefail

export XDG_CURRENT_DESKTOP=XFCE
export XDG_SESSION_DESKTOP=xfce
export XDG_SESSION_TYPE=x11

if [[ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]]; then
  eval "$(dbus-launch --sh-syntax --exit-with-session)"
fi

startxfce4 &
xfce_pid=$!

for _ in $(seq 1 20); do
  if xset q >/dev/null 2>&1; then
    xset s off
    xset -dpms
    xset s noblank
    break
  fi
  sleep 1
done

wait "$xfce_pid"

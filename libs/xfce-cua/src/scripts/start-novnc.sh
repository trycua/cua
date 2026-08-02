#!/bin/bash
set -euo pipefail

while ! nc -z 127.0.0.1 "${VNC_PORT:-5901}"; do
  sleep 1
done

exec websockify \
  --web=/usr/share/novnc \
  "${NOVNC_PORT:-6901}" \
  "127.0.0.1:${VNC_PORT:-5901}"

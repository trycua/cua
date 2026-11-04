#!/usr/bin/env bash
# Chromium in the cua desktop session, maximized, DevTools on 127.0.0.1:9223.
# Its profile lives in /home/cua/.config/bench-web-chromium; bench-web-ctl
# /reset clears cookies and storage and leaves one blank tab.
# --no-sandbox: gVisor and most container runtimes have no unprivileged user
# namespaces for Chromium's sandbox; the sandbox boundary is the sandbox itself.
set -euo pipefail
ENV_FILE="${CUA_DESKTOP_RUNTIME_DIR:-/run/cua-desktop}/desktop.env"
for _ in $(seq 1 120); do [ -r "$ENV_FILE" ] && break; sleep 1; done
if [ -r "$ENV_FILE" ]; then set -a; . "$ENV_FILE"; set +a; else export DISPLAY="${CUA_DISPLAY:-:1}"; fi
for _ in $(seq 1 120); do [ -S "/tmp/.X11-unix/X${DISPLAY#:}" ] && break; sleep 1; done
geom="$(xdpyinfo 2>/dev/null | awk '/dimensions:/ {print $2; exit}')"
geom="${geom:-1280x800}"
exec /usr/bin/chromium \
    --user-data-dir="$HOME/.config/bench-web-chromium" \
    --remote-debugging-address=127.0.0.1 --remote-debugging-port=9223 \
    --no-first-run --no-default-browser-check --disable-dev-shm-usage --no-sandbox \
    --disable-features=Translate,MediaRouter --disable-background-networking \
    --password-store=basic --window-position=0,0 --window-size="${geom/x/,}" --start-maximized \
    about:blank

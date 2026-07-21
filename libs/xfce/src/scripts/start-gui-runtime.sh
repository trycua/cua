#!/bin/bash
set -euo pipefail

echo "Waiting for X server to start..."
while ! xdpyinfo -display :1 >/dev/null 2>&1; do
    sleep 1
done
echo "X server is ready"

export DISPLAY=:1
case "${CUA_GUI_RUNTIME:-computer-server}" in
    computer-server)
        exec /opt/venv/bin/python3 -m computer_server --port "${API_PORT:-8000}"
        ;;
    cua-driver)
        # The SDK and CLI use this daemon's default per-user socket. There is
        # deliberately no TCP listener or computer-server compatibility API.
        # Linux has no macOS permissions gate; standard is the daemon's default
        # authorization mode and admits the portable desktop tools.
        exec /opt/venv/bin/cua-driver serve --permission-mode standard
        ;;
    *)
        echo "CUA_GUI_RUNTIME must be computer-server or cua-driver" >&2
        exit 2
        ;;
esac

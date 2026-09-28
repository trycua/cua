#!/usr/bin/env bash
# Expose `cua-driver mcp` (stdio) as MCP Streamable HTTP on 0.0.0.0:3000/mcp,
# driving the OSWorld image's auto-login Xorg session (user `user`).
set -euo pipefail
DISPLAY_NUM=""
for _ in $(seq 1 120); do
    SOCK=$(ls /tmp/.X11-unix/ 2>/dev/null | head -n 1 || true)
    if [ -n "$SOCK" ]; then DISPLAY_NUM=":${SOCK#X}"; break; fi
    sleep 1
done
export DISPLAY="${DISPLAY_NUM:-:0}"
export XAUTHORITY="${XAUTHORITY:-$HOME/.Xauthority}"
export PATH="/usr/local/bin:/opt/node/bin:$PATH"
exec /usr/local/bin/supergateway \
    --stdio "/usr/local/bin/cua-driver mcp" \
    --outputTransport streamableHttp --stateful --sessionTimeout 3600000 \
    --port 3000 --streamableHttpPath /mcp --healthEndpoint /healthz --cors

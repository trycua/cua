#!/bin/bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
#
# The agent of one Linux XFCE trial: Claude Code `claude -p` with the driver's MCP server(s) and no other tools,
# exactly the invocation of the macOS, Windows and Hyprland release runs. Run it as the COMMAND of xfce_session.py
# (the session sets CDB_BRIEF_FILE and runs it from the trial's output directory):
#
#   python3 launch/xfce_session.py --task IR-02 --run-index 0 --condition background --out /tmp/trial \
#       --label cua-driver --timeout 360 -- launch/xfce/agent.sh cua-driver
#
# ARM is cua-driver or agent-desktop. The agent needs the claude CLI (Claude Code 2.1.316), ANTHROPIC_API_KEY or a
# logged-in session, and the desktop's environment (DISPLAY, DBUS_SESSION_BUS_ADDRESS; AT-SPI running).
set -euo pipefail
ARM=${1:?usage: agent.sh cua-driver|agent-desktop}
CDB_HOME=${CDB_HOME:-$HOME/cdb}
BRIEF=${CDB_BRIEF_FILE:?CDB_BRIEF_FILE is set by xfce_session.py}
CLAUDE=${CLAUDE:-claude}
MODEL=${MODEL:-claude-sonnet-5-5}
TURNS=${TURNS:-45}

case "$ARM" in
  cua-driver)
    export CUA_DRIVER_RS_TELEMETRY_ENABLED=false DO_NOT_TRACK=1
    cat > mcp.json <<JSON
{"mcpServers": {"cua": {"command": "$CDB_HOME/cua-driver-rs-0.38.0-linux-x86_64/cua-driver", "args": ["mcp"]}}}
JSON
    ALLOWED=mcp__cua ;;
  agent-desktop)
    export AGENT_DESKTOP_TELEMETRY=0 DO_NOT_TRACK=1
    AD=$CDB_HOME/agent-desktop/node_modules/agent-desktop/bin/agent-desktop-linux-x64
    # Two servers, as on Windows and Hyprland: strict headless (default) and --headed; the agent picks per call.
    cat > mcp.json <<JSON
{"mcpServers": {"agentdesktop": {"command": "$AD", "args": ["mcp"]},
                "agentdesktop_headed": {"command": "$AD", "args": ["mcp", "--headed"]}}}
JSON
    ALLOWED=mcp__agentdesktop,mcp__agentdesktop_headed ;;
  *) echo "unknown arm $ARM" >&2; exit 2 ;;
esac

export MAX_THINKING_TOKENS=16000 DISABLE_AUTOUPDATER=1 CLAUDE_CODE_DISABLE_AUTO_MEMORY=1 \
       CLAUDE_CODE_DISABLE_CLAUDE_MDS=1 DISABLE_TELEMETRY=1
mkdir -p cwd && cd cwd
exec "$CLAUDE" -p "$(cat "$BRIEF")" --model "$MODEL" --effort medium \
  --mcp-config ../mcp.json --strict-mcp-config --tools "" --allowed-tools "$ALLOWED" \
  --permission-mode bypassPermissions --setting-sources project --max-turns "$TURNS" \
  --output-format stream-json --verbose > ../agent-transcript.jsonl 2> ../agent-stderr.log

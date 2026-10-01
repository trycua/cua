#!/bin/bash
# Cuabot container entrypoint
# Adds skills on startup (they go to /home/user/.claude which is mounted)

# Add skills in background if not already present
if [ ! -f /home/user/.claude/.skills-installed ]; then
    (
        echo "[entrypoint] Installing skills in background..."
        npx skills add callstackincubator/agent-device --all 2>/dev/null || true
        npx skills add vercel-labs/agent-browser --all 2>/dev/null || true
        touch /home/user/.claude/.skills-installed
        echo "[entrypoint] Skills installed"
    ) &
fi

# Start dbus and xpra
dbus-daemon --session --fork --address=unix:path=/tmp/dbus-session 2>/dev/null || true
exec xpra seamless :100 --sharing=yes --no-daemon --bind-tcp=0.0.0.0:10000 --html=on --dpi=96

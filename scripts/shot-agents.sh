#!/usr/bin/env bash
# Recapture apps/cua-spaces/docs/images/spaces-list-agents.png.
#
# Renders the REAL main window (MainWindow, SpaceWindowList,
# SpaceAgentList, TeleportDropZone, the real stylesheet) over data captured live
# from a Space — see src/devtools/agentsShot.tsx for what is real and what is
# stubbed. Offscreen: nothing appears on the operator's desktop.
#
# To refresh the DATA, re-capture src/devtools/realRuns.json (the output of
# agents::parse_runs over a Space's record stream) and realWindows.json first;
# this script only re-renders.
set -euo pipefail
cd "$(dirname "$0")/../apps/cua-spaces"
CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
OUT="docs/images/spaces-list-agents.png"

npx vite --port 5199 --strictPort >/tmp/vite-shot.log 2>&1 &
VITE=$!
trap 'kill $VITE 2>/dev/null || true' EXIT
until grep -q "ready in" /tmp/vite-shot.log; do sleep 1; done

"$CHROME" --headless=new --disable-gpu --hide-scrollbars \
  --force-device-scale-factor=2 --window-size=1040,740 \
  --virtual-time-budget=9000 --screenshot="$OUT" \
  "http://localhost:5199/shot.html" >/dev/null 2>&1
echo "wrote apps/cua-spaces/$OUT"

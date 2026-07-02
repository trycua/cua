#!/bin/bash
MODE="${1:-ax-bg}"
DRV="$HOME/cua/libs/cua-driver/rust/target/release/cua-driver"
export NO_AT_BRIDGE=0 GTK_A11Y=1 GDK_BACKEND=x11 CUA_DRIVER_RS_DRAW_SYSTEM_CURSOR=0
pkill Xvfb 2>/dev/null; pkill cua-driver 2>/dev/null; pkill picom 2>/dev/null
pkill -f 'http.server' 2>/dev/null; fuser -k 8146/tcp 2>/dev/null
pkill -f lin-harness 2>/dev/null; pkill -f lin-dash 2>/dev/null
sleep 1.5
rm -f "/tmp/cua-lin-$MODE/setup.log" /tmp/cua-lin-state.json
xvfb-run -a --server-args="-screen 0 1024x768x24" dbus-run-session -- bash -c '
  export GDK_BACKEND=x11 CUA_DRIVER_RS_DRAW_SYSTEM_CURSOR=0
  DRV="'"$DRV"'"
  openbox & sleep 1
  picom --backend xrender --config /dev/null >/tmp/picom.log 2>&1 & sleep 1
  xdotool mousemove 1010 758   # park the real X pointer in the corner (only the agent cursor should show)
  "$DRV" serve >"/tmp/cua-lin-'"$MODE"'-drv.log" 2>&1 & sleep 3
  python3 /tmp/lin-rec.py "'"$MODE"'"
  pkill cua-driver; pkill picom; pkill openbox
' > "/tmp/cua-lin-$MODE-run.log" 2>&1
echo "exit=$? mode=$MODE"

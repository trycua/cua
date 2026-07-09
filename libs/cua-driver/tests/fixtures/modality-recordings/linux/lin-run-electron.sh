#!/bin/bash
# Env wrapper for the Electron modality recorder — mirrors lin-run.sh but
# launches lin-rec-electron.py (which spawns the Electron harness itself).
MODE="${1:-ax-bg}"
DRV="$HOME/cua/libs/cua-driver/rust/target/release/cua-driver"
export NO_AT_BRIDGE=0 GTK_A11Y=1 GDK_BACKEND=x11 CUA_DRIVER_RS_DRAW_SYSTEM_CURSOR=0
pkill Xvfb 2>/dev/null; pkill cua-driver 2>/dev/null; pkill picom 2>/dev/null
pkill -f 'http.server' 2>/dev/null; fuser -k 8146/tcp 2>/dev/null
pkill -x electron 2>/dev/null; pkill -f lin-dash 2>/dev/null
sleep 1.5
rm -f "/tmp/cua-lin-electron-$MODE/setup.log"
xvfb-run -a --server-args="-screen 0 1024x768x24" dbus-run-session -- bash -c '
  export GDK_BACKEND=x11 CUA_DRIVER_RS_DRAW_SYSTEM_CURSOR=0
  DRV="'"$DRV"'"
  # bring up the AT-SPI a11y bus so Chromium can register its web-AX tree
  for p in /usr/libexec/at-spi-bus-launcher /usr/lib/at-spi2-core/at-spi-bus-launcher; do
    [ -x "$p" ] && "$p" --launch-immediately & break
  done
  sleep 1
  openbox & sleep 1
  picom --backend xrender --config /dev/null >/tmp/picom.log 2>&1 & sleep 1
  xdotool mousemove 1010 758
  "$DRV" serve >"/tmp/cua-lin-electron-'"$MODE"'-drv.log" 2>&1 & sleep 3
  python3 /tmp/lin-rec-electron.py "'"$MODE"'"
  pkill -x electron; pkill cua-driver; pkill picom; pkill openbox
' > "/tmp/cua-lin-electron-$MODE-run.log" 2>&1
echo "exit=$? mode=$MODE"

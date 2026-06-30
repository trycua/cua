#!/bin/bash
# cua-driver desktop modality + recording harness — the SSH/VM lane.
#
# Companion to the container lane in this directory: where calc.sh /
# modality_matrix.sh drive ACI containers over the computer-server / `az exec`
# (XFCE, GTK3 galculator), derec.sh drives full Azure VMs over SSH and is the
# only lane that exercises the genuinely different desktops/toolkits:
#   - KDE Plasma (X11) with kcalc      (Qt)
#   - GNOME Shell (X11, console Xorg)  with gnome-calculator (GTK4)
# It auto-discovers the live X session env (DISPLAY/XAUTHORITY/dbus) from a
# running DE process, so the same script works on any of them.
#
#   derec.sh setup            # fresh daemon + launch $APP + snapshot AT-SPI tree
#   derec.sh verify           # assert the coordinate invariant (see below)
#   derec.sh record i.. i..   # record start_recording -> element clicks -> stop
#   derec.sh env              # print the discovered session env
#
# APP defaults to gnome-calculator; set APP=kcalc / galculator for the others.
#
# COORDINATE INVARIANT (the GTK4 regression guard):
#   GTK4's AT-SPI returns GetExtents(SCREEN) as (0,0) for every widget, which
#   used to collapse every element `frame` to the window corner. The fix queries
#   CoordType::Window + reconstructs screen = x11_origin + _GTK_FRAME_EXTENTS +
#   WINDOW. `verify` asserts the observable invariant: per-button frames are
#   DISTINCT (not collapsed) and every button center lies inside the window's
#   X11 rect, with "7" left of "8" on the same row when both exist.
export HOME=/home/fbonacci
W=$HOME/derec; mkdir -p "$W"
DE_PID=""
for p in gnome-shell plasmashell kwin_x11 metacity xfce4-session; do
  DE_PID=$(pgrep -x "$p" | head -1); [ -n "$DE_PID" ] && break
done
if [ -n "$DE_PID" ]; then
  while IFS= read -r kv; do export "$kv"; done < <(tr '\0' '\n' </proc/$DE_PID/environ 2>/dev/null | grep -E '^(DISPLAY|XAUTHORITY|DBUS_SESSION_BUS_ADDRESS|XDG_RUNTIME_DIR)=')
fi
[ -n "$DISP" ] && export DISPLAY="$DISP"
export DISPLAY="${DISPLAY:-:0}"
CUA=$HOME/.local/bin/cua-driver
APP="${APP:-gnome-calculator}"; SESS=demo

resolve_pid(){ python3 -c "import json,re
t=open('$W/launch.json').read(); p=None
try: p=json.loads(t).get('pid')
except: pass
if not p:
 m=re.search(r'pid (\d+)',t); p=m.group(1) if m else ''
print(p or '')"; }

case "$1" in
 env) echo "DISPLAY=$DISPLAY XAUTHORITY=$XAUTHORITY dbus=${DBUS_SESSION_BUS_ADDRESS:+set} APP=$APP DE_PID=$DE_PID($p)";;
 setup)
  pkill -f 'cua-driver serve' 2>/dev/null; pkill -f "$APP" 2>/dev/null; sleep 2
  rm -f ~/.cache/cua-driver/cua-driver.sock
  (setsid "$CUA" serve >$W/cuad.log 2>&1 &); sleep 2
  "$CUA" call launch_app "{\"name\":\"$APP\"}" >$W/launch.json 2>&1; sleep 4
  PID=$(resolve_pid); echo "$PID" >$W/pid
  "$CUA" call list_windows "{\"pid\":$PID}" >$W/win.json 2>&1
  WID=$(python3 -c "import json;ws=json.load(open('$W/win.json')).get('windows',[]);print(ws[0]['window_id'] if ws else '')" 2>/dev/null)
  echo "$WID" >$W/wid
  echo "app=$APP pid=$PID wid=$WID DISPLAY=$DISPLAY"
  "$CUA" call get_window_state "{\"pid\":$PID,\"window_id\":$WID,\"capture_mode\":\"ax\"}" >$W/state.json 2>&1
  python3 -c "import json
d=json.load(open('$W/state.json')); els=d.get('elements',[])
print('elements',len(els),'degraded',d.get('degraded'))
for e in els:
    l=e.get('label')
    if l and len(str(l))<=4: print(e.get('element_index'),repr(l))" 2>&1 | head -40
  ;;
 verify) # assert the coordinate invariant against the X11 window geometry
  WID=$(cat $W/wid)
  read WX WY WW WH < <(xwininfo -id "$WID" 2>/dev/null | awk '/Absolute upper-left X/{x=$NF}/Absolute upper-left Y/{y=$NF}/Width:/{w=$NF}/Height:/{h=$NF}END{print x,y,w,h}')
  FE=$(xprop -id "$WID" _GTK_FRAME_EXTENTS 2>/dev/null | grep -oE '= .*' | tr -d ' =')
  echo "app=$APP window=($WX,$WY) ${WW}x${WH} _GTK_FRAME_EXTENTS=${FE:-absent}"
  python3 - "$WX" "$WY" "$WW" "$WH" "$W/state.json" <<'PY'
import json,sys
wx,wy,ww,wh=map(int,sys.argv[1:5]); d=json.load(open(sys.argv[5]))
btns=[]
for e in d.get('elements',[]):
    l=str(e.get('label')); r=str(e.get('role','')).lower(); f=e.get('frame')
    if f and 'button' in r and len(l)<=3:
        btns.append((l, f['x']+f['w']//2, f['y']+f['h']//2))
fails=[]
xs={cx for _,cx,_ in btns}
if len(btns) >= 3 and len(xs) < 2:
    fails.append(f'COLLAPSED: {len(btns)} buttons all at x={xs} (GTK4 (0,0) regression)')
for l,cx,cy in btns:
    if not (wx-2 <= cx <= wx+ww+2 and wy-2 <= cy <= wy+wh+2):
        fails.append(f'OUT-OF-WINDOW: {l!r} center=({cx},{cy}) outside [{wx},{wx+ww}]x[{wy},{wy+wh}]')
g={l:(cx,cy) for l,cx,cy in btns}
if '7' in g and '8' in g:
    (x7,y7),(x8,y8)=g['7'],g['8']
    if not (x8 > x7 and abs(y8-y7) <= 6):
        fails.append(f"ROW: 7={g['7']} 8={g['8']} not left-to-right on one row")
print(f'buttons checked: {len(btns)}')
print('COORD INVARIANT:', 'PASS' if not fails else 'FAIL')
for f in fails[:8]: print('  -', f)
sys.exit(0 if not fails else 1)
PY
  ;;
 record) # record <idx>...  start_recording -> element clicks (in order) -> stop
  PID=$(cat $W/pid); WID=$(cat $W/wid); shift
  rm -rf $W/rec; mkdir -p $W/rec
  "$CUA" call start_session "{\"session\":\"$SESS\"}" >/dev/null 2>&1
  "$CUA" call start_recording "{\"output_dir\":\"$W/rec\",\"record_video\":true}" 2>&1 | head -1
  sleep 2
  for idx in "$@"; do
    "$CUA" call click "{\"pid\":$PID,\"window_id\":$WID,\"element_index\":$idx,\"delivery_mode\":\"background\",\"session\":\"$SESS\"}" >/dev/null 2>&1
    sleep 1.3
  done
  sleep 1.5
  "$CUA" call stop_recording '{}' 2>&1 | head -1
  ls -la $W/rec/recording.mp4 2>&1 | tail -1
  ;;
 *) echo "usage: derec.sh setup|verify|record <idx>...|env  (APP=gnome-calculator|kcalc|galculator)";;
esac

#!/usr/bin/env bash
# cua-driver modality test harness for the XFCE-in-a-container lane.
#
# Drives galculator through the four parity modalities (background AX click,
# background pixel/vision click, foreground EWMH type, background type
# focus-limit) so each `delivery_mode` rung can be validated against a real WM
# (xfwm4 + EWMH) and a live AT-SPI tree. Invoke ONE action per call; state
# (pid/window) persists in /tmp/cstate.* between calls.
#
# Usage:  bash calc.sh <action> [args...]
# See README.md in this directory for the full lane (image, install, gotchas).

export DISPLAY=:1
export HOME=/home/cua
export XDG_RUNTIME_DIR=/run/user/1000

# AT-SPI needs the desktop session bus. The cua-driver daemon now AUTO-DISCOVERS
# DBUS_SESSION_BUS_ADDRESS at startup (platform-linux/src/session_bus.rs), so the
# `serve` process self-recovers it even when started outside the session. We
# still export it here for the short-lived `cua-driver call` CLI invocations and
# for `gsettings`, discovering it from a running session process's environ the
# same way the daemon does (the VNC/XFCE session runs an ad-hoc bus).
for _p in xfce4-session xfsettingsd xfwm4 xfdesktop Thunar; do
  _pid=$(pgrep -x "$_p" | head -1)
  if [ -n "$_pid" ] && [ -r /proc/$_pid/environ ]; then
    _a=$(tr '\0' '\n' < /proc/$_pid/environ | sed -n 's/^DBUS_SESSION_BUS_ADDRESS=//p' | head -1)
    [ -n "$_a" ] && export DBUS_SESSION_BUS_ADDRESS="$_a" && break
  fi
done
export XAUTHORITY=/home/cua/.Xauthority
CUA=/home/cua/.local/bin/cua-driver
S=/tmp/cstate
act="$1"; shift

case "$act" in
  daemon)
    pgrep -f 'cua-driver serve' >/dev/null || (setsid "$CUA" serve >/tmp/cuad.log 2>&1 &)
    sleep 2
    "$CUA" status 2>&1 | head -4
    ;;
  doctor)
    # Exercises the hardened AT-SPI probe (org.a11y.Bus name_has_owner +
    # discovered DBUS_SESSION_BUS_ADDRESS).
    "$CUA" call check_permissions '{}' 2>&1 | head -20
    ;;
  a11y)
    gsettings set org.gnome.desktop.interface toolkit-accessibility true 2>/dev/null
    pgrep -f at-spi-bus-launcher >/dev/null || (setsid /usr/libexec/at-spi-bus-launcher --launch-immediately >/tmp/atspi.log 2>&1 &)
    sleep 1
    pkill -x galculator 2>/dev/null; sleep 1
    GTK_MODULES=gail:atk-bridge NO_AT_BRIDGE=0 setsid galculator >/dev/null 2>&1 &
    sleep 2
    pgrep -x galculator | head -1 > "$S.pid"
    echo "a11y on; galculator pid=$(cat $S.pid)  atspi=$(pgrep -f at-spi-bus-launcher | head -1)"
    ;;
  state)   # snapshot the AT-SPI tree; prints element map (and degraded flag)
    PID=$(cat "$S.pid")
    [ -z "$PID" ] && PID=$(pgrep -x galculator | head -1) && echo "$PID" > "$S.pid"
    WID=$("$CUA" call list_windows "{\"pid\":$PID}" 2>/dev/null | python3 -c "import sys,json;ws=json.load(sys.stdin).get('windows',[]);print(ws[0]['window_id'] if ws else '')" 2>/dev/null)
    echo "$WID" > "$S.wid"
    "$CUA" call get_window_state "{\"pid\":$PID,\"window_id\":$WID,\"capture_mode\":\"ax\"}" > /tmp/state.json 2>&1
    echo "pid=$PID wid=$WID"
    python3 - <<'PY'
import json
try:
    d=json.load(open('/tmp/state.json'))
except Exception as e:
    print("STATE PARSE FAIL:", str(e)[:200]); print(open('/tmp/state.json').read()[:400]); raise SystemExit
if d.get('degraded'):
    print("DEGRADED:", d.get('degraded_reason','')[:160])
els=d.get('elements',[])
print("total elements:", len(els))
for e in els:
    r=e.get('role',''); l=e.get('label')
    if r and ('button' in r.lower() or 'push' in r.lower() or l):
        print(e.get('element_index'), r, repr(l)[:22])
PY
    ;;
  click)   # click <element_index> <background|foreground>
    PID=$(cat "$S.pid"); WID=$(cat "$S.wid")
    "$CUA" call click "{\"pid\":$PID,\"window_id\":$WID,\"element_index\":$1,\"delivery_mode\":\"$2\"}"
    ;;
  pxclick) # pxclick <label> <background|foreground> — click a button by PIXEL coords (window-local)
    PID=$(cat "$S.pid"); WID=$(cat "$S.wid")
    read LX LY SX SY < <(python3 - "$1" <<'PY'
import json, sys
label = sys.argv[1]
st = json.load(open('/tmp/state.json'))
win = json.load(open('/tmp/win.json'))['windows'][0]
wx, wy = win['x'], win['y']
for e in st.get('elements', []):
    if str(e.get('label')) == label and e.get('frame'):
        f = e['frame']; cx = f['x'] + f['w']/2; cy = f['y'] + f['h']/2
        print(int(cx - wx), int(cy - wy), int(cx), int(cy)); break
PY
)
    echo "label=$1 screen=($SX,$SY) win-local=($LX,$LY)"
    "$CUA" call click "{\"pid\":$PID,\"window_id\":$WID,\"x\":$LX,\"y\":$LY,\"delivery_mode\":\"$2\"}"
    ;;
  type)    # type <text> <background|foreground>
    PID=$(cat "$S.pid")
    "$CUA" call type_text "{\"pid\":$PID,\"text\":\"$1\",\"delivery_mode\":\"$2\"}"
    ;;
  btf)
    PID=$(cat "$S.pid"); WID=$(cat "$S.wid")
    "$CUA" call bring_to_front "{\"pid\":$PID,\"window_id\":$WID}"
    ;;
  prep)    # resolve window id into /tmp/win.json + cstate.wid (needed before pxclick)
    PID=$(cat "$S.pid")
    [ -z "$PID" ] && PID=$(pgrep -x galculator | head -1) && echo "$PID" > "$S.pid"
    echo "pid=$PID"
    "$CUA" call list_windows "{\"pid\":$PID}" | tee /tmp/win.json
    WID=$(python3 -c "import json;ws=json.load(open('/tmp/win.json')).get('windows',[]);print(ws[0]['window_id'] if ws else '')" 2>/dev/null)
    echo "$WID" > "$S.wid"; echo "wid=$WID"
    ;;
  fullreset)
    # kill daemon (reaps zombie galculator children) + all galculator, enable
    # a11y, restart, launch. Use when list_windows comes back empty (a zombie
    # unreaped child of the daemon pollutes pgrep).
    pkill -9 -f 'cua-driver serve' 2>/dev/null; pkill -9 -f galculator 2>/dev/null; sleep 3
    rm -f /home/cua/.cache/cua-driver/cua-driver.sock
    gsettings set org.gnome.desktop.interface toolkit-accessibility true 2>/dev/null
    pgrep -f at-spi-bus-launcher >/dev/null || (setsid /usr/libexec/at-spi-bus-launcher --launch-immediately >/tmp/atspi.log 2>&1 &)
    sleep 1
    (setsid "$CUA" serve >/tmp/cuad.log 2>&1 &); sleep 2
    "$CUA" call launch_app '{"name":"galculator"}' > /tmp/launch.json 2>&1
    sleep 3
    python3 - <<'PY'
import json,re
t=open('/tmp/launch.json').read(); p=None
try: p=json.loads(t).get('pid')
except Exception: pass
if not p:
    m=re.search(r'pid (\d+)',t); p=m.group(1) if m else ''
open('/tmp/cstate.pid','w').write(str(p or ''))
PY
    echo "fullreset: galculator pid=$(cat $S.pid) remaining_galc=$(pgrep -x galculator|tr '\n' ' ')"
    ;;
  *)
    echo "unknown action: $act"; echo "actions: daemon doctor a11y state click pxclick type btf prep fullreset"
    ;;
esac

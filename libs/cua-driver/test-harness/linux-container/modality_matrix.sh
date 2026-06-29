#!/usr/bin/env bash
# Full modality matrix for the XFCE container, driven via `az container exec`
# (positional args only — the JSON is built inside the script). Proves the
# session-bus auto-discovery fix (the daemon is started with
# DBUS_SESSION_BUS_ADDRESS UNSET) and exercises every input tool across
# background/foreground × ax/vision. Screenshots land in /tmp/mm_*.png.
#
#   modality_matrix.sh setup        # fresh daemon (NO dbus env) + a11y + galculator + snapshot
#   modality_matrix.sh matrix       # run every tool × bg/fg, print path+result
#   modality_matrix.sh shot <name>  # screenshot -> /tmp/mm_<name>.png
#   modality_matrix.sh b64 <name>   # print a saved screenshot as base64 (host decodes)
set -uo pipefail
export DISPLAY=:1
export HOME=/home/cua
export XAUTHORITY=/home/cua/.Xauthority
CUA=/home/cua/.local/bin/cua-driver
S=/tmp/cstate

pidf() { cat "$S.pid" 2>/dev/null; }
widf() { cat "$S.wid" 2>/dev/null; }

snap() {  # snapshot AT-SPI tree -> /tmp/state.json, print element count + degraded
  local PID WID; PID=$(pidf); WID=$(widf)
  "$CUA" call get_window_state "{\"pid\":$PID,\"window_id\":$WID,\"capture_mode\":\"ax\"}" >/tmp/state.json 2>&1
  python3 - <<'PY'
import json
try: d=json.load(open('/tmp/state.json'))
except Exception as e: print("  STATE PARSE FAIL:",str(e)[:120]); raise SystemExit
els=d.get('elements',[])
print(f"  AT-SPI elements={len(els)} degraded={d.get('degraded')}")
if d.get('degraded'): print("  degraded_reason:",str(d.get('degraded_reason'))[:120])
PY
}

call() {  # call <label> <tool> <json> ; prints "<label> | path=.. ok=.."
  local label="$1" tool="$2" body="$3"
  "$CUA" call "$tool" "$body" >/tmp/last.json 2>&1
  python3 - "$label" <<'PY'
import json,sys
lbl=sys.argv[1]
try: d=json.load(open('/tmp/last.json'))
except Exception:
    print(f"  {lbl:30s} | RAW: "+open('/tmp/last.json').read()[:90].replace(chr(10),' ')); raise SystemExit
path=d.get('path') or d.get('delivery_path') or '-'
ok=d.get('ok', d.get('success', d.get('clicked', d.get('typed','?'))))
ver=d.get('verified')
extra=f" verified={ver}" if ver is not None else ""
print(f"  {lbl:30s} | path={path:14s} ok={ok}{extra}")
PY
}

case "${1:-help}" in
  setup)
    pkill -9 -f 'cua-driver serve' 2>/dev/null; pkill -9 -f galculator 2>/dev/null; sleep 2
    rm -f /home/cua/.cache/cua-driver/cua-driver.sock
    gsettings set org.gnome.desktop.interface toolkit-accessibility true 2>/dev/null
    pgrep -f at-spi-bus-launcher >/dev/null || (setsid /usr/libexec/at-spi-bus-launcher --launch-immediately >/tmp/atspi.log 2>&1 &)
    sleep 1
    # THE TEST: start the daemon with DBUS_SESSION_BUS_ADDRESS removed from its
    # environment. If AT-SPI still populates below, auto-discovery worked.
    env -u DBUS_SESSION_BUS_ADDRESS setsid "$CUA" serve >/tmp/cuad.log 2>&1 &
    sleep 3
    echo "=== daemon started with DBUS_SESSION_BUS_ADDRESS UNSET ==="
    echo "daemon env DBUS set? $(env -u DBUS_SESSION_BUS_ADDRESS bash -c 'echo ${DBUS_SESSION_BUS_ADDRESS:-<unset>}')"
    grep -iE "session bus|adopted the desktop|DBUS_SESSION" /tmp/cuad.log | head -3 || echo "(no discovery log line)"
    "$CUA" call launch_app '{"name":"galculator"}' >/tmp/launch.json 2>&1
    sleep 3
    PID=$(python3 -c "import json,re;t=open('/tmp/launch.json').read()
try: p=json.loads(t).get('pid')
except Exception: p=None
if not p:
 import re; m=re.search(r'pid (\d+)',t); p=m.group(1) if m else ''
print(p or '')")
    echo "$PID" > "$S.pid"
    "$CUA" call list_windows "{\"pid\":$PID}" >/tmp/win.json 2>&1
    WID=$(python3 -c "import json;ws=json.load(open('/tmp/win.json')).get('windows',[]);print(ws[0]['window_id'] if ws else '')" 2>/dev/null)
    echo "$WID" > "$S.wid"
    echo "galculator pid=$PID wid=$WID"
    echo "=== AT-SPI tree (proves auto-discovery if elements>0) ==="
    snap
    ;;

  matrix)
    PID=$(pidf); WID=$(widf)
    echo "=== pid=$PID wid=$WID ==="
    # resolve a couple of element indices + their pixel centers from the tree
    read -r E7 PX7 PY7 E_PLUS < <(python3 - <<'PY'
import json
d=json.load(open('/tmp/state.json')); win=json.load(open('/tmp/win.json'))['windows'][0]
def find(lbl):
    for e in d.get('elements',[]):
        if str(e.get('label'))==lbl: return e
    return None
e7=find('7'); ep=find('+')
def ctr(e):
    f=e.get('frame');
    return (int(f['x']+f['w']/2), int(f['y']+f['h']/2)) if f else (-1,-1)
x7,y7=ctr(e7) if e7 else (-1,-1)
print(e7.get('element_index') if e7 else -1, x7, y7, ep.get('element_index') if ep else -1)
PY
)
    echo "indices: 7->$E7 (px $PX7,$PY7)  +->$E_PLUS"
    echo "--- AX modality (element_index) ---"
    call "click 7 [ax/bg]"        click        "{\"pid\":$PID,\"window_id\":$WID,\"element_index\":$E7,\"delivery_mode\":\"background\"}"
    call "click + [ax/fg]"        click        "{\"pid\":$PID,\"window_id\":$WID,\"element_index\":$E_PLUS,\"delivery_mode\":\"foreground\"}"
    call "double_click 7 [ax/bg]" double_click "{\"pid\":$PID,\"window_id\":$WID,\"element_index\":$E7,\"delivery_mode\":\"background\"}"
    call "right_click 7 [ax/bg]"  right_click  "{\"pid\":$PID,\"window_id\":$WID,\"element_index\":$E7,\"delivery_mode\":\"background\"}"
    echo "--- VISION modality (pixel coords) ---"
    call "click px7 [vision/bg]"  click        "{\"pid\":$PID,\"window_id\":$WID,\"x\":$PX7,\"y\":$PY7,\"delivery_mode\":\"background\"}"
    call "click px7 [vision/fg]"  click        "{\"pid\":$PID,\"window_id\":$WID,\"x\":$PX7,\"y\":$PY7,\"delivery_mode\":\"foreground\"}"
    call "double_click px7 [v/bg]" double_click "{\"pid\":$PID,\"window_id\":$WID,\"x\":$PX7,\"y\":$PY7,\"delivery_mode\":\"background\"}"
    echo "--- keyboard / scroll ---"
    call "type 789 [bg]"          type_text    "{\"pid\":$PID,\"text\":\"789\",\"delivery_mode\":\"background\"}"
    call "type 789 [fg]"          type_text    "{\"pid\":$PID,\"text\":\"789\",\"delivery_mode\":\"foreground\"}"
    call "press_key Return [bg]"  press_key    "{\"pid\":$PID,\"window_id\":$WID,\"key\":\"Return\",\"delivery_mode\":\"background\"}"
    call "press_key Return [fg]"  press_key    "{\"pid\":$PID,\"window_id\":$WID,\"key\":\"Return\",\"delivery_mode\":\"foreground\"}"
    call "hotkey ctrl+c [bg]"     hotkey       "{\"pid\":$PID,\"window_id\":$WID,\"keys\":[\"ctrl\",\"c\"],\"delivery_mode\":\"background\"}"
    call "scroll down [bg]"       scroll       "{\"pid\":$PID,\"window_id\":$WID,\"direction\":\"down\",\"amount\":3,\"delivery_mode\":\"background\"}"
    call "bring_to_front (EWMH)"  bring_to_front "{\"pid\":$PID,\"window_id\":$WID}"
    ;;

  shot)
    # Grab the X display directly (no `screenshot` tool; ffmpeg x11grab is present).
    ffmpeg -y -loglevel error -f x11grab -i :1 -frames:v 1 "/tmp/mm_${2:-shot}.png" 2>/tmp/shot.err
    ls -la "/tmp/mm_${2:-shot}.png" 2>&1 | tail -1
    [ -s /tmp/shot.err ] && head -c 200 /tmp/shot.err
    ;;

  b64) base64 -w0 "/tmp/mm_${2:-shot}.png" 2>/dev/null ;;

  *) echo "usage: modality_matrix.sh setup|matrix|shot <name>|b64 <name>" ;;
esac

#!/usr/bin/env bash
# End-to-end smoke test for linux (container rootfs variant).
#
# Starts the image under the chosen OCI runtime (runc, or runsc = gVisor) and
# exercises the real surfaces:
#   supervisord   desktop / cua-spacesd programs RUNNING
#   X + XFCE      Xvfb :1 answers xdpyinfo, xfce4-panel mapped
#   frames        an in-guest X capture of the root window -> evidence PNG,
#                 with the clicked grid cell's color checked
#   viewer        GET /viewer/ (the cua-spacesd HTML5 viewer) from the host
#   fixtures      grid/form/http start; xdotool click on grid cell (2,3) shows
#                 up in grid.jsonl with cell [2,3]; the pixel under the click
#                 has the cell's deterministic color; typing into the form's
#                 Name entry logs entry_changed; HTTP /health + /bytes sha256
#   AT-SPI        the form's "Submit" push button is visible over the a11y bus
#   audio         PipeWire up with default sink cua_desktop / source cua_mic;
#                 the tone fixture (440/880 Hz) is heard on cua_desktop.monitor;
#                 a 660 Hz tone played into cua_mic_in comes out of cua_mic
#                 (uplink); the A/V sync fixture's flash and beep for the same
#                 second land within 40 ms of each other
#   Browser       Firefox (full) or Chromium (slim) opens the HTTP fixture page
#   spacesd    token materialised at /run/cua/env-token (0640, == the one
#                 passed in), never in any argv; :3211 listens when the
#                 driver is installed (images built with SOURCE=none skip it)
#
# Usage: smoke-test.sh [--image REF] [--runtime runc|runsc] [--evidence DIR] [--keep]
#   SMOKE_SKIP_BROWSER=1  skip the browser check
# The VM (containerDisk) variant is exercised by ../common/boot-qemu.sh.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
host_arch() { case "$(uname -m)" in arm64|aarch64) echo arm64 ;; *) echo amd64 ;; esac; }
IMAGE="cua-e2e-local/linux:docker-local-$(host_arch)"
RUNTIME=runc
EVIDENCE="${SMOKE_EVIDENCE:-$PWD/smoke-evidence}"
KEEP=0
while [ $# -gt 0 ]; do
    case "$1" in
        --image) IMAGE="$2"; shift 2 ;;
        --runtime) RUNTIME="$2"; shift 2 ;;
        --evidence) EVIDENCE="$2"; shift 2 ;;
        --keep) KEEP=1; shift ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
mkdir -p "$EVIDENCE"
NAME="cua-e2e-images-desktop-$RUNTIME-$$"
TOKEN="smoke-$(date +%s)-$RANDOM$RANDOM"
PASS=0; FAIL=0
t_start=$SECONDS

ok() { PASS=$((PASS + 1)); echo "  ok   $*"; }
bad() { FAIL=$((FAIL + 1)); echo "  FAIL $*"; }
check() { local d="$1"; shift; if "$@" >/dev/null 2>&1; then ok "$d"; else bad "$d"; fi; }
dx() { docker exec "$NAME" "$@"; }
dxd() { docker exec "$NAME" desktop-env "$@"; }
cleanup() {
    docker logs "$NAME" >"$EVIDENCE/$RUNTIME-container.log" 2>&1 || true
    docker exec "$NAME" sh -c 'tail -n 200 /var/log/supervisor/*.log; cat /tmp/cua-fixtures/*.jsonl' \
        >"$EVIDENCE/$RUNTIME-guest-logs.txt" 2>&1 || true
    [ "$KEEP" = 1 ] || docker rm -f "$NAME" >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "==> $IMAGE under --runtime=$RUNTIME"
docker run -d --name "$NAME" --runtime="$RUNTIME" --shm-size=512m \
    --memory="${SMOKE_MEMORY:-4g}" --memory-swap="${SMOKE_MEMORY:-4g}" \
    -e CUA_ENV_TOKEN="$TOKEN" \
    -p 127.0.0.1::3211 "$IMAGE" >/dev/null
port() { docker port "$NAME" "$1/tcp" | head -1 | sed 's/.*://'; }
ENV_PORT="$(port 3211)"
# Frame of the root window, taken inside the guest and copied out.
# (cat, not docker cp: gVisor keeps the sandbox's writes out of docker cp's view)
shot() { dxd import -window root "/tmp/$1" && docker exec "$NAME" cat "/tmp/$1" >"$EVIDENCE/$RUNTIME-$1"; }
pixel() { dxd convert "/tmp/$1" -crop "1x1+$2+$3" -depth 8 txt:- | sed -n '2s/.*srgb(\([0-9]*\),\([0-9]*\),\([0-9]*\)).*/\1 \2 \3/p'; }

echo "==> waiting for healthy"
status=""
for _ in $(seq 1 90); do
    status="$(docker inspect -f '{{.State.Health.Status}}' "$NAME" 2>/dev/null || echo gone)"
    [ "$status" = healthy ] && break
    [ "$status" = gone ] && break
    sleep 1
done
[ "$status" = healthy ] && ok "container healthy after $((SECONDS - t_start))s" || { bad "container health=$status"; exit 1; }
echo "    runtime inside: $(dx sh -c 'dmesg 2>/dev/null | head -1 | cut -c1-60 || true')"

echo "==> services"
sv=""
for _ in $(seq 1 20); do
    sv="$(dx supervisorctl status || true)"
    grep -Eq '(STARTING|BACKOFF)' <<<"$sv" || break
    sleep 1
done
for p in desktop audio cua-spacesd dbus-system; do
    grep -Eq "^$p +RUNNING" <<<"$sv" && ok "supervisor $p RUNNING" || bad "supervisor $p: $(grep "^$p" <<<"$sv")"
done
check "X display :1 answers" dxd xdpyinfo
check "X server is Xvfb" bash -c "docker exec $NAME pgrep -x Xvfb"
check "no VNC server or websockify" bash -c "! docker exec $NAME pgrep -f 'Xvnc|websockify'"
for _ in $(seq 1 20); do dxd wmctrl -l 2>/dev/null | grep -q xfce4-panel && break; sleep 1; done
check "xfce4-panel mapped" bash -c "docker exec $NAME desktop-env wmctrl -l | grep -q xfce4-panel"

echo "==> HTML5 viewer from the host"
if dx test -x /usr/local/bin/cua-spacesd; then
    for _ in $(seq 1 30); do curl -fsS "http://127.0.0.1:$ENV_PORT/viewer/" >/dev/null 2>&1 && break; sleep 1; done
    check "GET /viewer/ serves the viewer" bash -c "curl -fsS http://127.0.0.1:$ENV_PORT/viewer/ | grep -q viewer.js"
else
    echo "  skip /viewer (image built without the driver)"
fi

echo "==> fixtures"
if dx cua-fixtures start >"$EVIDENCE/$RUNTIME-fixtures-start.txt" 2>&1; then ok "cua-fixtures start"; else bad "cua-fixtures start"; cat "$EVIDENCE/$RUNTIME-fixtures-start.txt"; fi
check "grid window mapped" bash -c "docker exec $NAME desktop-env wmctrl -l | grep -q 'CUA Fixture Grid'"
check "form window mapped" bash -c "docker exec $NAME desktop-env wmctrl -l | grep -q 'CUA Fixture Form'"

# Click the centre of grid cell (2,3), measured from the grid's client origin.
read -r GX GY < <(dxd bash -c 'wid=$(xdotool search --name "CUA Fixture Grid" | head -1); xdotool windowactivate --sync "$wid"; xdotool windowraise "$wid"; sleep 0.5; xwininfo -id "$wid" | awk "/Absolute upper-left X/{x=\$4} /Absolute upper-left Y/{y=\$4} END{print x, y}"')
CX=$((GX + 2 * 80 + 40)); CY=$((GY + 3 * 80 + 40))
dxd xdotool mousemove "$CX" "$CY" click 1
sleep 0.5
check "grid logged button_press in cell [2,3]" bash -c \
    "docker exec $NAME grep '\"type\": \"button_press\"' /tmp/cua-fixtures/grid.jsonl | grep -q '\"cell\": \\[2, 3\\]'"

# Frame of the X display, then check the clicked cell's color: (2*255//7, 3*255//5, 128).
# Sampled up-left of the click point, clear of the pointer.
if shot desktop.png; then
    ok "frame captured -> $EVIDENCE/$RUNTIME-desktop.png"
    px="$(pixel desktop.png $((CX - 20)) $((CY - 20)))"
    [ "$px" = "72 153 128" ] && ok "grid pixel color matches cell (2,3): $px" || bad "grid pixel: got '$px' want '72 153 128'"
else
    bad "frame capture (import -window root)"
fi

read -r FX FY < <(dxd bash -c 'wid=$(xdotool search --name "CUA Fixture Form" | head -1); xdotool windowactivate --sync "$wid"; sleep 0.3; xwininfo -id "$wid" | awk "/Absolute upper-left X/{x=\$4} /Absolute upper-left Y/{y=\$4} END{print x, y}"')
dxd xdotool mousemove $((FX + 120)) $((FY + 25)) click 1
dxd xdotool type --delay 30 "smoke"
sleep 0.5
check "form logged entry_changed 'smoke'" bash -c \
    "docker exec $NAME grep -q '\"text\": \"smoke\", \"ts\"' /tmp/cua-fixtures/form.jsonl || docker exec $NAME grep '\"entry_changed\"' /tmp/cua-fixtures/form.jsonl | grep -q '\"smoke\"'"
check "http fixture /health" bash -c "docker exec $NAME curl -fsS http://127.0.0.1:18080/health | grep -q ok"
want_sha="$(python3 -c 'import hashlib;print(hashlib.sha256(bytes(i%251 for i in range(100000))).hexdigest())')"
check "http fixture /bytes/100000 sha256" bash -c "docker exec $NAME sh -c 'curl -fsS http://127.0.0.1:18080/bytes/100000 | sha256sum' | grep -q $want_sha"

echo "==> AT-SPI"
docker exec -i "$NAME" desktop-env python3 - >"$EVIDENCE/$RUNTIME-atspi.txt" 2>&1 <<'PY' || true
import pyatspi
desk = pyatspi.Registry.getDesktop(0)
found = []
def walk(acc, depth=0):
    if depth > 12: return
    try:
        if acc.getRoleName() == "push button" and acc.name == "Submit":
            found.append(acc)
        for i in range(acc.childCount):
            walk(acc.getChildAtIndex(i), depth + 1)
    except Exception:
        pass
for app in desk:
    if app is not None and app.name == "cua-fixture-form":
        walk(app)
print("apps:", [a.name for a in desk if a is not None])
print("submit_buttons:", len(found))
PY
grep -q "submit_buttons: 1" "$EVIDENCE/$RUNTIME-atspi.txt" && ok "AT-SPI finds form 'Submit' push button" || bad "AT-SPI: $(cat "$EVIDENCE/$RUNTIME-atspi.txt")"

echo "==> audio"
pinfo="$(dxd pactl info 2>&1 || true)"
grep -q '^Server Name: PulseAudio (on PipeWire' <<<"$pinfo" && ok "pipewire-pulse answers" || bad "pactl info: $pinfo"
grep -q '^Default Sink: cua_desktop$' <<<"$pinfo" && ok "default sink cua_desktop" || bad "default sink: $(grep 'Default Sink' <<<"$pinfo")"
grep -q '^Default Source: cua_mic$' <<<"$pinfo" && ok "default source cua_mic" || bad "default source: $(grep 'Default Source' <<<"$pinfo")"
# tone was started by `cua-fixtures start` (all); 440/880 each play 1/3 of a cycle.
probe="$(dxd python3 /opt/cua/fixtures/audio_probe.py --source cua_desktop.monitor --seconds 3 --freq 440 --freq 880 2>&1 || true)"
echo "$probe" >"$EVIDENCE/$RUNTIME-audio-desktop.json"
python3 - "$probe" <<'PY' && ok "tone fixture captured on cua_desktop.monitor: $probe" || bad "desktop capture: $probe"
import json, sys
p = json.loads(sys.argv[1].strip().splitlines()[-1])
t = p["tones"]
sys.exit(0 if p["rms_dbfs"] is not None and p["rms_dbfs"] > -30
         and t["440"]["dominant_fraction"] >= 0.2 and t["880"]["dominant_fraction"] >= 0.2 else 1)
PY
probe="$(dxd bash -c 'python3 -c "
import math, struct, sys, wave
w = wave.open(\"/tmp/smoke-uplink.wav\", \"wb\"); w.setnchannels(1); w.setsampwidth(2); w.setframerate(48000)
w.writeframes(b\"\".join(struct.pack(\"<h\", int(12000 * math.sin(2 * math.pi * 660 * i / 48000))) for i in range(48000 * 4)))
" && (pacat --playback --device=cua_mic_in --file-format=wav /tmp/smoke-uplink.wav &) && sleep 0.5 &&
    python3 /opt/cua/fixtures/audio_probe.py --source cua_mic --seconds 2 --freq 660 --freq 440' 2>&1 || true)"
echo "$probe" >"$EVIDENCE/$RUNTIME-audio-uplink.json"
python3 - "$probe" <<'PY' && ok "uplink: 660 Hz into cua_mic_in recorded from cua_mic" || bad "uplink: $probe"
import json, sys
p = json.loads(sys.argv[1].strip().splitlines()[-1])
sys.exit(0 if p["rms_dbfs"] is not None and p["tones"]["660"]["dominant_fraction"] >= 0.8 else 1)
PY

if [ "${SMOKE_SKIP_BROWSER:-0}" != 1 ]; then
    # Firefox in the full tier, Chromium in slim (which has no Firefox).
    if dx sh -c 'command -v firefox' >/dev/null 2>&1; then
        browser=Firefox cmd='firefox --new-instance'
    else
        browser=Chromium cmd='/opt/cua/bin/cua-chromium --no-first-run --no-default-browser-check'
    fi
    echo "==> $browser"
    dxd bash -c "setsid $cmd http://127.0.0.1:18080/ >/tmp/browser.log 2>&1 &"
    ff=0
    for _ in $(seq 1 45); do
        if dxd wmctrl -l 2>/dev/null | grep -q "CUA Fixture Page"; then ff=1; break; fi
        sleep 1
    done
    [ "$ff" = 1 ] && ok "$browser mapped 'CUA Fixture Page'" || bad "$browser window not seen: $(dx tail -3 /tmp/browser.log 2>/dev/null)"
    sleep 8  # let the page paint past the browser's startup splash
    shot "$(echo "$browser" | tr '[:upper:]' '[:lower:]').png" || true
fi

echo "==> A/V sync fixture"
dx cua-fixtures start avsync >/dev/null 2>&1 || true
sleep 4
dx cat /tmp/cua-fixtures/avsync.jsonl >"$EVIDENCE/$RUNTIME-avsync.jsonl" 2>/dev/null || true
dx cua-fixtures stop avsync >/dev/null 2>&1 || true
python3 - "$EVIDENCE/$RUNTIME-avsync.jsonl" <<'PY' >"$EVIDENCE/$RUNTIME-avsync.txt" 2>&1 && ok "avsync: $(cat "$EVIDENCE/$RUNTIME-avsync.txt")" || bad "avsync: $(cat "$EVIDENCE/$RUNTIME-avsync.txt")"
import json, sys
ev = {}
for line in open(sys.argv[1]):
    r = json.loads(line)
    if r["type"] in ("beep", "flash_drawn"):
        ev.setdefault(r["boundary"], {})[r["type"]] = r["mono"]
skews = [round((e["flash_drawn"] - e["beep"]) * 1000, 1) for e in ev.values() if len(e) == 2]
print(f"{len(skews)} flash/beep pairs, skew ms {skews}")
sys.exit(0 if len(skews) >= 2 and all(abs(s) <= 40 for s in skews) else 1)
PY

echo "==> spacesd token"
check "token file == CUA_ENV_TOKEN" bash -c "[ \"\$(docker exec $NAME cat /run/cua/env-token)\" = '$TOKEN' ]"
check "token file mode 640 root:cua" bash -c "docker exec $NAME stat -c '%a %U:%G' /run/cua/env-token | grep -qx '640 root:cua'"
check "token not in any argv" bash -c "! docker exec $NAME sh -c 'cat /proc/[0-9]*/cmdline 2>/dev/null | tr \"\\0\" \" \"' | grep -q '$TOKEN'"
if dx test -x /usr/local/bin/cua-spacesd; then
    listening=0
    for _ in $(seq 1 30); do dx ss -Hltn "sport = :3211" | grep -q . && { listening=1; break; }; sleep 1; done
    [ "$listening" = 1 ] && ok "cua-spacesd listening on :3211" || bad "cua-spacesd not listening on :3211"
else
    echo "  skip cua-spacesd :3211 (image built without the driver: $(dx cat /etc/cua-image/spacesd-source))"
fi

echo "==> $PASS passed, $FAIL failed in $((SECONDS - t_start))s ($RUNTIME, $IMAGE)"
[ "$FAIL" = 0 ]

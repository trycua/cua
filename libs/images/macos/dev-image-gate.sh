#!/usr/bin/env bash
# Live gate for the macOS Space with developer tools (opt-in; needs Apple
# silicon, lume and a derived base from derive-local.sh): boot a Space cloned
# from the derived base and, inside it, build and test an iOS app on the
# Simulator, build a Rust core for the iOS Simulator target, run the app and
# take real screenshots. Writes timings, logs and pictures to --out.
#
#   libs/images/macos/dev-image-gate.sh --base-vm NAME --out DIR [--name VM] [--keep]
#
#   --base-vm  the derived base VM (derive-local.sh --name)
#   --out      results: timings.json, tools.txt, logs, simulator.png, desktop.png,
#              ui-screenshots/ (taken by the UI test)
#   --name     the Space's VM name (default cua-dev-gate-<pid>); removed at the end
#              unless --keep
#
# CUA_LUME_BIN / CUA_LUME_STORAGE select the lume binary and storage directory,
# as for derive-local.sh.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BASE_VM="" OUT="" VM="cua-dev-gate-$$" KEEP=0
while [ $# -gt 0 ]; do
    case "$1" in
        --base-vm) BASE_VM="$2"; shift 2 ;;
        --out) OUT="$2"; shift 2 ;;
        --name) VM="$2"; shift 2 ;;
        --keep) KEEP=1; shift ;;
        -h|--help) sed -n '2,17p' "$0"; exit 0 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
[ -n "$BASE_VM" ] && [ -n "$OUT" ] || { echo "--base-vm and --out are required" >&2; exit 2; }
ST="${CUA_LUME_STORAGE:-}"
LUME_BIN="${CUA_LUME_BIN:-$(type -P lume || echo lume)}"
lume() {
    local sub="$1"; shift
    case "$sub" in
        clone) if [ -n "$ST" ]; then "$LUME_BIN" clone --source-storage "$ST" --dest-storage "$ST" "$@"; else "$LUME_BIN" clone "$@"; fi ;;
        run|ssh|get|set|stop|delete) if [ -n "$ST" ]; then "$LUME_BIN" "$sub" --storage "$ST" "$@"; else "$LUME_BIN" "$sub" "$@"; fi ;;
        *) "$LUME_BIN" "$sub" "$@" ;;
    esac
}
guest() { lume ssh -t "${2:-300}" "$VM" "$1" </dev/null; }
log() { echo "[dev-image-gate $(date +%T)] $*" >&2; }
now() { date +%s; }

mkdir -p "$OUT"
STAGE="$OUT/stage"
rm -rf "$STAGE"; mkdir -p "$STAGE/fixture" "$STAGE/results"
cp -R "$HERE/fixtures/HelloSim/." "$STAGE/fixture/"
cp "$HERE/files/dev-image-gate-guest.sh" "$STAGE/"

cleanup() {
    lume stop "$VM" >/dev/null 2>&1 || true
    for pid in $(pgrep -f "lume run .*$VM" || true); do kill "$pid" 2>/dev/null || true; done
    [ "$KEEP" = 1 ] || lume delete --force "$VM" >/dev/null 2>&1 || true
}
trap cleanup EXIT

t0=$(now)
log "cloning $BASE_VM -> $VM"
lume clone "$BASE_VM" "$VM"
t_clone=$(now)
lume set --memory 8GB "$VM"
log "booting"
lume run --display none --detach --shared-dir "$STAGE" --log-file "$OUT/vm.log" "$VM" >/dev/null
up=0
for _ in $(seq 1 120); do guest 'echo up' 10 2>/dev/null | grep -q up && { up=1; break; }; sleep 2; done
[ "$up" = 1 ] || { echo "$VM never answered over ssh (see $OUT/vm.log)" >&2; exit 1; }
t_ssh=$(now)
for _ in $(seq 1 60); do guest 'pgrep -qx Dock && pgrep -qx Finder' 10 2>/dev/null && break; sleep 2; done
t_desktop=$(now)

S="/Volumes/My Shared Files/$(basename "$STAGE")"
log "build, test and drive the app in the Simulator"
guest "bash '$S/dev-image-gate-guest.sh' '$S'" 3600 2>&1 | sed 's/^/    /' >&2
t_guest=$(now)

log "desktop screenshot (the Simulator on the Space's screen)"
guest "uid=\$(id -u); sudo -n launchctl asuser \$uid /usr/sbin/screencapture -x '$S/results/desktop.png' || printf '%s\n' lume | sudo -S -p '' launchctl asuser \$uid /usr/sbin/screencapture -x '$S/results/desktop.png'" 60 || true
guest 'df -h / | tail -1' 30 >"$STAGE/results/guest-df.txt" || true

cp -R "$STAGE/results/." "$OUT/"
rm -rf "$STAGE"
size="$(lume get --format json "$VM" | python3 -c 'import json,sys;d=json.load(sys.stdin);d=d[0] if isinstance(d,list) else d;print(d["diskSize"]["allocated"])')"
python3 - "$OUT/timings.txt" "$OUT/timings.json" "$((t_clone - t0))" "$((t_ssh - t_clone))" "$((t_desktop - t_clone))" "$((t_guest - t_desktop))" "$size" <<'PY'
import json, sys
src, dst, clone, ssh, desktop, guest, size = sys.argv[1:]
d = {k: int(v) for k, v in (l.split() for l in open(src).read().splitlines() if l.strip())}
d.update(clone_seconds=int(clone), first_boot_to_ssh_seconds=int(ssh), first_boot_to_desktop_seconds=int(desktop),
         guest_gate_seconds=int(guest), space_disk_allocated_bytes=int(size))
json.dump(d, open(dst, "w"), indent=2)
print(json.dumps(d, indent=2))
PY
log "done: $OUT"

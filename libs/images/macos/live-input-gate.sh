#!/usr/bin/env bash
# Live input gate of a built macOS tier: clone the gated (stopped, sanitized)
# VM, boot the clone with a token in the Lume setup share (as the cua SDK
# starts a Space), and run libs/cua/crates/cua-spaces-ext/tests/e2e_macos_input.rs
# against it: a whole-display stream with the viewers' desktop policy, a Dock
# click that must be acknowledged as delivered and change the screen, and the
# SwiftUI viewer's wire shape. The clone is always stopped and deleted, so the
# VM that gets pushed never boots again after its sanitize.
#
#   live-input-gate.sh --vm NAME --out DIR [--memory 4GB] [--timeout 900]
#
# Writes DIR/test.txt (the cargo test output) and DIR/input.json. Exit 0 only
# when both tests ran (not skipped) and passed. Builds the test with cargo
# (CARGO_TARGET_DIR as set). macOS allows two VMs per host; one lane at a time.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
VM="" OUT="" MEMORY=4GB TIMEOUT=900
while [ $# -gt 0 ]; do
    case "$1" in
        --vm) VM="$2"; shift 2 ;;
        --out) OUT="$2"; shift 2 ;;
        --memory) MEMORY="$2"; shift 2 ;;
        --timeout) TIMEOUT="$2"; shift 2 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
[ -n "$VM" ] && [ -n "$OUT" ] || { echo "--vm and --out are required" >&2; exit 2; }
command -v lume >/dev/null || { echo "lume is required" >&2; exit 2; }
mkdir -p "$OUT"
NAME="cua-e2e-input-$$"
SETUP="$(mktemp -d "${TMPDIR:-/tmp}/cua-input-gate.XXXXXX")/setup"
mkdir -m 700 "$SETUP"
(umask 077; openssl rand -hex 32 >"$SETUP/env-token")
log() { echo "[macos-input $(date +%T)] $*" >&2; }
cleanup() {
    rm -rf "$(dirname "$SETUP")"
    lume stop "$NAME" >/dev/null 2>&1 || true
    local pid
    for pid in $(pgrep -f "lume run $NAME --display none" || true); do kill "$pid" 2>/dev/null || true; done
    lume delete "$NAME" --force >/dev/null 2>&1 || true
}
trap cleanup EXIT

# Build the test first, so the clone does not idle while cargo compiles.
log "building the e2e_macos_input test"
(cd "$REPO/libs/cua" && cargo test -p cua-spaces-ext --test e2e_macos_input --no-run) >&2

log "cloning $VM -> $NAME"
lume clone "$VM" "$NAME"
lume set "$NAME" --memory "$MEMORY"
lume run "$NAME" --display none --detach --shared-dir "$SETUP:ro" --log-file "$OUT/lume.log" >/dev/null
guest() { lume ssh "$NAME" -t "${2:-60}" "$1" </dev/null; }
up=0
for _ in $(seq 1 $((TIMEOUT / 5))); do guest 'echo up' 2>/dev/null | grep -q up && { up=1; break; }; sleep 5; done
[ "$up" = 1 ] || { echo "$NAME never answered over ssh" >&2; exit 2; }
# The login session (Dock, Finder) and the LaunchAgent's service.
for _ in $(seq 1 60); do guest 'pgrep -qx Dock && pgrep -qx Finder' 2>/dev/null && break; sleep 5; done
ready=0
for _ in $(seq 1 60); do
    if guest 'curl -s -o /dev/null -m 3 -w "%{http_code}" http://127.0.0.1:3211/health' 2>/dev/null | grep -q '^204'; then
        ready=1; break
    fi
    sleep 5
done
[ "$ready" = 1 ] || { echo "cua-spacesd /health never answered 204 in $NAME" >&2; exit 2; }
# The first login indexes what the tier added; let the load settle (bounded)
# so the click is not starved.
for _ in $(seq 1 30); do
    load="$(guest 'sysctl -n vm.loadavg' 15 2>/dev/null | awk 'NR==1 {print int($2)}' | tr -dc '0-9')"
    [ -n "$load" ] && [ "$load" -lt 6 ] && break
    sleep 10
done
ip="$(lume get "$NAME" --format json | python3 -c 'import json,sys;d=json.load(sys.stdin);d=d[0] if isinstance(d,list) else d;print(d.get("ipAddress") or "")')"
[ -n "$ip" ] || { echo "$NAME has no IP" >&2; exit 2; }
log "running e2e_macos_input against http://$ip:3211 (1-min load ${load:-?})"

set +e
(cd "$REPO/libs/cua" &&
    CUA_SPACES_MACOS_E2E_URL="http://$ip:3211" CUA_SPACES_MACOS_E2E_TOKEN="$(cat "$SETUP/env-token")" \
        timeout "$TIMEOUT" cargo test -p cua-spaces-ext --test e2e_macos_input -- --test-threads=1 --nocapture) \
    >"$OUT/test.txt" 2>&1
rc=$?
set -e
tail -25 "$OUT/test.txt" >&2
passed="$(sed -n 's/^test result: [a-zA-Z]*\. \([0-9]*\) passed.*/\1/p' "$OUT/test.txt" | tail -1)"
skipped=0
grep -q '^skipped: set CUA_SPACES_MACOS_E2E_URL' "$OUT/test.txt" && skipped=1
status=fail
[ "$rc" = 0 ] && [ "$skipped" = 0 ] && [ "${passed:-0}" -ge 2 ] && status=pass
python3 -c 'import json,sys;json.dump({"lane":"lume-input","arch":"arm64","vm":sys.argv[1],"clone":sys.argv[2],"test":"libs/cua/crates/cua-spaces-ext/tests/e2e_macos_input.rs","exit":int(sys.argv[3]),"passed":int(sys.argv[4] or 0),"status":sys.argv[5]},open(sys.argv[6],"w"),indent=1)' \
    "$VM" "$NAME" "$rc" "${passed:-0}" "$status" "$OUT/input.json"
log "live input: $status (${passed:-0} passed, exit $rc)"
[ "$status" = pass ]

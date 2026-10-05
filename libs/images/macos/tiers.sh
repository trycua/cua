#!/usr/bin/env bash
# Build the macOS tiers in order, each on the VM of the tier below that just
# passed `doctor --strict` (never a re-pull of a published tag):
#
#   base --(build.sh --tier slim)--> slim --(full)--> full --(xcode)--> xcode
#
#   libs/images/macos/tiers.sh [--upto slim|full|xcode] [--base-vm NAME | --base-image REF]
#       [--app "PATH/Cua Spacesd.app"] [--stamp <yyyymmdd>-<sha7>] [--xcode-tag]
#       [--prefix NAME] [--keep] [--out DIR]
#
#   --upto       the last tier to build (default full)
#   --stamp      push each tier after its gate as an immutable pin:
#                26-slim-<stamp>, 26-<stamp>, 26-xcode-<X.Y>-<stamp>
#                (moving tags move later, through check-tag-safety.sh --moving)
#   --prefix     VM name prefix (default cua-e2e-macos-<pid>); VMs are
#                <prefix>-<tier>
#   --keep       keep every tier's VM (default: all deleted at exit)
#   --out        reports per tier under DIR/<tier> (default
#                ~/.cache/cua-images/macos-tiers)
#
# The app is built once (unless --app) so every tier carries the same
# cua-spacesd, and each tier's doctor expects it.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
UPTO=full BASE=() APP="" STAMP="" PREFIX="cua-e2e-macos-$$" KEEP=0
OUT="${CUA_IMAGES_OUT:-$HOME/.cache/cua-images}/macos-tiers"
while [ $# -gt 0 ]; do
    case "$1" in
        --upto) UPTO="$2"; shift 2 ;;
        --base-vm|--base-image) BASE=("$1" "$2"); shift 2 ;;
        --app) APP="$2"; shift 2 ;;
        --stamp) STAMP="$2"; shift 2 ;;
        --prefix) PREFIX="$2"; shift 2 ;;
        --keep) KEEP=1; shift ;;
        --out) OUT="$2"; shift 2 ;;
        -h|--help) sed -n '2,24p' "$0"; exit 0 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
case "$UPTO" in slim) TIERS=(slim) ;; full) TIERS=(slim full) ;; xcode) TIERS=(slim full xcode) ;;
    *) echo "--upto is slim, full or xcode" >&2; exit 2 ;; esac
[ -z "$STAMP" ] || [[ "$STAMP" =~ ^[0-9]{8}-[0-9a-f]{7}$ ]] || { echo "--stamp is <yyyymmdd>-<sha7>" >&2; exit 2; }
# shellcheck source=versions.env
. "$HERE/versions.env"
log() { echo "[macos-tiers $(date +%T)] $*" >&2; }

built=()
cleanup() {
    [ "$KEEP" = 1 ] && return 0
    local vm
    for vm in ${built[@]+"${built[@]}"}; do lume delete "$vm" --force >/dev/null 2>&1 || true; done
}
trap cleanup EXIT

expect=()
if [ -z "$APP" ]; then
    log "building cua-spacesd (release) and Cua Spacesd.app once for every tier"
    (cd "$REPO/libs/cua-spacesd" && cargo build -p cua-spacesd --release)
    bash "$REPO/libs/cua-spacesd/scripts/build-macos-app.sh" >/dev/null
    APP="${CARGO_TARGET_DIR:-$REPO/libs/cua-spacesd/target}/macos/Cua Spacesd.app"
    expect=(--expect-git "$(git -C "$REPO" rev-parse HEAD)")
fi

base=(${BASE[@]+"${BASE[@]}"})
for tier in "${TIERS[@]}"; do
    vm="$PREFIX-$tier"
    push=()
    if [ -n "$STAMP" ]; then
        case "$tier" in
            slim) push=(--push "26-slim-$STAMP") ;;
            full) push=(--push "26-$STAMP") ;;
            xcode) push=(--push "26-xcode-$XCODE_VERSION-$STAMP") ;;
        esac
    fi
    log "tier $tier -> $vm"
    built+=("$vm")
    "$HERE/build.sh" --tier "$tier" --name "$vm" --keep --app "$APP" --out "$OUT/$tier" \
        ${base[@]+"${base[@]}"} ${expect[@]+"${expect[@]}"} ${push[@]+"${push[@]}"}
    base=(--base-vm "$vm")
    lume get "$vm" --format json | python3 -c '
import json, sys
d = json.load(sys.stdin); d = d[0] if isinstance(d, list) else d
print(json.dumps({"vm": d["name"], "disk_used_bytes": d["diskSize"]["allocated"], "disk_total_bytes": d["diskSize"]["total"]}))
' >"$OUT/$tier/size.json" 2>/dev/null || true
    log "tier $tier passed ($(cat "$OUT/$tier/size.json" 2>/dev/null))"
done
log "done: ${built[*]}"

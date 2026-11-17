#!/usr/bin/env bash
# Derive the macOS Space with developer tools (the xcode tier) on THIS Mac,
# from the published full tier and an Xcode that is already here. The default
# way to get the tier is to pull the published ghcr.io/trycua/macos:26-xcode;
# this is the fallback for when that image is not available, or for a Mac that
# already has the Xcode it wants and the user prefers to use it. Xcode and the
# iOS Simulator runtimes are Apple software under Apple's license, for use on
# Apple-branded hardware: this refuses to run anywhere else.
#
#   libs/images/macos/derive-local.sh --base-vm NAME [--name VM] [--xcode PATH]
#       [--out DIR] [--memory 8GB] [--keep-going-low-disk] [--json]
#
#   --base-vm NAME  the local Lume VM of the full tier to clone (the SDK keeps
#                   it as cua-base-<sha12>); it is never modified
#   --name VM       the derived VM (default cua-dev-macos-<pid>)
#   --xcode PATH    an Xcode.app or Xcode .xip to use; default: the Mac's own
#                   (xcode-select, then /Applications/Xcode*.app, newest first,
#                   then an .xip in $CUA_XCODE_CACHE, ~/XcodesCache or
#                   ~/Downloads)
#   --out DIR       staging and logs (default ~/.cache/cua-images/macos-dev)
#   --memory        memory for the build boot (default 8GB; the VM keeps the
#                   base's own value afterwards)
#   --json          print one JSON line of results (sizes and timings) at the end
#
# Exit codes: 0 done; 2 usage; 3 no Xcode on this Mac (Apple needs a sign-in to
# download one: install it from the App Store or download the .xip from
# developer.apple.com, then run again); 4 not enough free disk; 5 the guest's
# macOS is older than the Xcode needs.
#
# CUA_LUME_BIN is the lume binary (default: lume on PATH).
# CUA_LUME_STORAGE is a Lume storage directory to use instead of the default
# one (every lume call gets --storage with it): the base VM must be there, and
# so is the derived VM.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BASE_VM="" VM="cua-dev-macos-$$" XCODE="" MEMORY=8GB JSON=0
OUT="${CUA_IMAGES_OUT:-$HOME/.cache/cua-images}/macos-dev"
# The volume must keep this much free after the derive (the build stops there).
FLOOR_GB="${CUA_DEV_IMAGE_FLOOR_GB:-10}"
# Growth the derive needs beyond Xcode itself: iOS runtime, Metal toolchain,
# tools, Simulator data and caches while building.
EXTRA_GB="${CUA_DEV_IMAGE_EXTRA_GB:-18}"
# shellcheck disable=SC2034 # XCODE is read by find_xcode (files/xcode-source.sh)
while [ $# -gt 0 ]; do
    case "$1" in
        --base-vm) BASE_VM="$2"; shift 2 ;;
        --name) VM="$2"; shift 2 ;;
        --xcode) XCODE="$2"; shift 2 ;;
        --out) OUT="$2"; shift 2 ;;
        --memory) MEMORY="$2"; shift 2 ;;
        --json) JSON=1; shift ;;
        -h|--help) sed -n '2,29p' "$0"; exit 0 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
[ -n "$BASE_VM" ] || { echo "--base-vm is required (the local VM of the full tier)" >&2; exit 2; }
# shellcheck source=versions.env
. "$HERE/versions.env"
log() { echo "[macos-dev $(date +%T)] $*" >&2; }
ST="${CUA_LUME_STORAGE:-}"
LUME_BIN="${CUA_LUME_BIN:-$(type -P lume || echo lume)}"
# lume SUBCOMMAND ARGS...: with CUA_LUME_STORAGE set, on that storage.
lume() {
    local sub="$1"; shift
    case "$sub" in
        clone) if [ -n "$ST" ]; then "$LUME_BIN" clone --source-storage "$ST" --dest-storage "$ST" "$@"; else "$LUME_BIN" clone "$@"; fi ;;
        run|ssh|get|set|stop|delete) if [ -n "$ST" ]; then "$LUME_BIN" "$sub" --storage "$ST" "$@"; else "$LUME_BIN" "$sub" "$@"; fi ;;
        *) "$LUME_BIN" "$sub" "$@" ;;
    esac
}
guest() { lume ssh -t "${2:-120}" "$VM" "$1" </dev/null; }
# shellcheck source=files/xcode-source.sh
. "$HERE/files/xcode-source.sh"
if ! SRC="$(find_xcode)"; then
    cat >&2 <<'MSG'
No Xcode on this Mac. Apple requires a signed-in download, so Cua cannot fetch one for you.
Install Xcode from the App Store, or download the Xcode .xip from developer.apple.com/download
and put it in ~/XcodesCache, then create the macOS Space with developer tools again.
MSG
    exit 3
fi
[ -e "$SRC" ] || { echo "$SRC does not exist" >&2; exit 2; }
log "Xcode source: $SRC"
case "$SRC" in
    *.app) XVER="$(plist "$SRC" CFBundleShortVersionString)"; XMIN="$(plist "$SRC" LSMinimumSystemVersion || true)"
           SRC_KB="$(( $(du -sk "$SRC" | cut -f1) * 2 ))" ;;  # the archive and the copy
    *.xip) XVER="$(basename "$SRC" | sed -n 's/^Xcode[_ -]\{0,1\}\([0-9][0-9.]*\).*/\1/p')"; XMIN=""
           SRC_KB="$(( $(stat -f %z "$SRC") * 3 / 1024 ))" ;;  # an .xip expands to about 3x
    *) echo "--xcode takes an Xcode.app or an .xip" >&2; exit 2 ;;
esac

# --- disk ---------------------------------------------------------------------------
LUME_DIR="${CUA_LUME_STORAGE:-$HOME/.lume}"
mkdir -p "$LUME_DIR" "$OUT"
free_gb() { df -k "$1" | awk 'NR==2 {print int($4/1024/1024)}'; }
need_gb=$(( SRC_KB / 1024 / 1024 + 1 + EXTRA_GB ))
have_gb="$(free_gb "$LUME_DIR")"
if [ "$have_gb" -lt $(( need_gb + FLOOR_GB )) ]; then
    cat >&2 <<MSG
Not enough free disk to build the macOS Space with developer tools: it needs about ${need_gb} GB
and the Mac should keep ${FLOOR_GB} GB free, but only ${have_gb} GB is free.
Make room: remove Spaces and images you no longer use, empty the Trash, or delete old Xcode caches
(~/Library/Developer/Xcode/DerivedData, ~/Library/Developer/CoreSimulator).
MSG
    exit 4
fi

[ "$(uname -s)" = Darwin ] && [ "$(uname -m)" = arm64 ] ||
    { echo "The macOS Space with developer tools runs only on Apple-branded hardware with Apple silicon." >&2; exit 2; }
command -v "$LUME_BIN" >/dev/null || { echo "lume is required" >&2; exit 2; }

# --- stage: scripts, pins, tools ----------------------------------------------------------
CACHE="${CUA_MACOS_CACHE:-$HOME/.cache/cua-images/macos-cache}"
SETUP="$OUT/setup"
rm -rf "$SETUP"; mkdir -p "$SETUP/cache" "$SETUP/xcode" "$CACHE"; chmod 700 "$SETUP"
cp "$HERE/files/tier-xcode.sh" "$HERE/versions.env" "$SETUP/"
# shellcheck source=files/fetch.sh
. "$HERE/files/fetch.sh"
fetch_xcode_tools
log "packing $(basename "$SRC") for the guest"
stage_xcode "$SRC" "$SETUP/xcode"

cleanup() {
    lume stop "$VM" >/dev/null 2>&1 || true
    for pid in $(pgrep -f "lume run .*$VM" || true); do kill "$pid" 2>/dev/null || true; done
    rm -rf "$SETUP"
    [ "${KEEP:-1}" = 1 ] || lume delete --force "$VM" >/dev/null 2>&1 || true
}
KEEP=0
trap cleanup EXIT

t0=$(date +%s)
log "cloning $BASE_VM -> $VM"
lume clone "$BASE_VM" "$VM"
base_memory="$(lume get --format json "$VM" | python3 -c 'import json,sys;d=json.load(sys.stdin);d=d[0] if isinstance(d,list) else d;print(d["memorySize"])')"
lume set --memory "$MEMORY" "$VM"
log "booting $VM"
lume run --display none --detach --shared-dir "$SETUP:ro" --log-file "$OUT/vm.log" "$VM" >/dev/null
up=0
for _ in $(seq 1 60); do guest 'echo up' 2>/dev/null | grep -q up && { up=1; break; }; sleep 5; done
[ "$up" = 1 ] || { echo "$VM never answered over ssh (see $OUT/vm.log)" >&2; exit 1; }
for _ in $(seq 1 36); do guest 'pgrep -qx Dock && pgrep -qx Finder' 2>/dev/null && break; sleep 5; done
t_boot=$(date +%s)
os="$(guest 'sw_vers -productVersion')"
if [ -n "${XMIN:-}" ] && [ "$(printf '%s\n%s\n' "$XMIN" "$os" | sort -V | head -1)" != "$XMIN" ]; then
    echo "Xcode $XVER needs macOS $XMIN; this Space's macOS is $os. Use an older Xcode (or a newer macOS image)." >&2
    exit 5
fi

S="/Volumes/My Shared Files/setup"
log "installing the developer tools (Xcode ${XVER:-?}, iOS runtime, tools)"
guest "bash '$S/tier-xcode.sh' '$S'" 5400 2>&1 | tee "$OUT/tier-xcode.log" | sed 's/^/    /' >&2
t_tier=$(date +%s)

log "shutting down"
guest "printf '%s\n' lume | sudo -S -p '' shutdown -h now" 30 >/dev/null 2>&1 || true
for _ in $(seq 1 60); do
    st="$(lume get --format json "$VM" 2>/dev/null | python3 -c 'import json,sys;d=json.load(sys.stdin);d=d[0] if isinstance(d,list) else d;print(d["status"])' 2>/dev/null || true)"
    [ "$st" = stopped ] && break; sleep 5
done
[ "$st" = stopped ] || lume stop "$VM" >/dev/null
lume set --memory "$((base_memory / 1024 / 1024))MB" "$VM"
KEEP=1
t_end=$(date +%s)
sizes="$(lume get --format json "$VM" | python3 -c 'import json,sys;d=json.load(sys.stdin);d=d[0] if isinstance(d,list) else d;print(json.dumps({"vm":d["name"],"disk_used_bytes":d["diskSize"]["allocated"],"disk_total_bytes":d["diskSize"]["total"]}))')"
log "derived $VM in $((t_end - t0)) s ($sizes)"
if [ "$JSON" = 1 ]; then
    python3 - "$sizes" "$XVER" "$((t_boot - t0))" "$((t_tier - t_boot))" "$((t_end - t0))" <<'PY'
import json, sys
d = json.loads(sys.argv[1]); d.update(xcode=sys.argv[2], clone_boot_seconds=int(sys.argv[3]), tier_seconds=int(sys.argv[4]), total_seconds=int(sys.argv[5]))
print(json.dumps(d))
PY
fi

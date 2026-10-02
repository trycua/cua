#!/usr/bin/env bash
# Build one tier of the canonical macOS Lume image (ghcr.io/trycua/macos) end
# to end, gated on `cua-spacesd doctor --strict` after a reboot. tiers.sh
# chains the tiers; each one clones the VM of the tier below that just passed.
#
#   clone base -> boot (setup share) -> install-guest.sh (app, LaunchAgent,
#   /etc/cua-image, TCC) -> tier-<tier>.sh -> reboot -> doctor-gate.sh
#   (strict) -> sanitize-guest.sh -> shut down -> [lume push]
#
#   | tier  | floating tag | base                      | adds (versions.env)                  |
#   |-------|--------------|---------------------------|--------------------------------------|
#   | slim  | 26-slim      | the Lume base (macos:26)  | cua-spacesd, Google Chrome           |
#   | full  | 26 (default) | the gated slim VM         | CLT, Homebrew, gh, jq, rg, Node,     |
#   |       |              |                           | pnpm, uv, Rust, Go                   |
#   | xcode | 26-xcode     | the gated full VM         | Xcode, iOS runtime, Metal toolchain  |
#
#   libs/images/macos/build.sh [--tier slim|full|xcode] [--version 26]
#       [--base-vm NAME | --base-image REF] [--app "PATH/Cua Spacesd.app"]
#       [--push TAG] [--keep] [--name VM] [--out DIR] [--memory 4GB]
#       [--expect-git SHA]
#
#   --tier         default slim. full and xcode need --base-vm (the tier below).
#   --base-vm      a local Lume VM to clone (e.g. the SDK's cached base,
#                  cua-base-<sha256(ref)[:12]>, or the gated tier below)
#   --base-image   pull this image instead (slim only; default
#                  MACOS_BASE_REPO:MACOS_BASE_TAG in versions.env, checked against
#                  MACOS_BASE_DIGEST)
#   --app          a prebuilt app bundle (default $CUA_MACOS_APP); default:
#                  build libs/cua-spacesd (cargo, release) and bundle it with
#                  build-macos-app.sh
#   --spacesd-source  local (default) or release (the bundle is a published
#                  cua-spacesd release asset; needs --app). Default
#                  $CUA_MACOS_SPACESD_SOURCE. Recorded in the image manifest
#                  and /etc/cua-image/spacesd-source. With --app the doctor
#                  expects the bundle's own build-info git_sha.
#   --push TAG     after the gate passes, `lume push` TAG to the macos repo.
#                  TAG must be a new immutable pin of this tier
#                  (<version>[-slim|-xcode[-X.Y]]-<yyyymmdd>-<sha7>);
#                  scripts/images/check-tag-safety.sh refuses anything else.
#                  Credentials: GITHUB_USERNAME / GITHUB_TOKEN (write:packages).
#   --keep         keep the build VM (default: deleted)
#   --name         the build VM's name (default cua-e2e-macos-<tier>-<pid>)
#   --memory       memory for the build boot (restored to the base's value
#                  before the push, so the image keeps its original config;
#                  default 4GB, 8GB for xcode)
#   --expect-git   the doctor expects cua-spacesd built from this commit
#                  (implied when build.sh builds the app itself)
#
# Downloads (Chrome, the full tier's archives) are cached in
# CUA_MACOS_CACHE (default ~/.cache/cua-images/macos-cache) and checked
# against versions.env on the host and again in the guest. The Xcode .xip
# must already be in CUA_XCODE_CACHE (default ~/XcodesCache): Apple needs a
# signed-in download.
#
# Signing: CUA_ENV_CODESIGN_IDENTITY (default ad-hoc). TCC rows are derived
# from the exact bundle installed (seed-tcc.sh), so an ad-hoc signature holds
# for that binary. Requires Apple Silicon, lume (with `lume serve`), crane.
# One VM at a time.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
VERSION=26 TIER=slim BASE_VM="" BASE_IMAGE="" APP="${CUA_MACOS_APP:-}" PUSH_TAG="" KEEP=0 MEMORY="" VM="" EXPECT_GIT=""
OUT="${CUA_IMAGES_OUT:-$HOME/.cache/cua-images}/macos"
SPACESD_SOURCE="${CUA_MACOS_SPACESD_SOURCE:-local}"
while [ $# -gt 0 ]; do
    case "$1" in
        --version) VERSION="$2"; shift 2 ;;
        --tier) TIER="$2"; shift 2 ;;
        --name) VM="$2"; shift 2 ;;
        --expect-git) EXPECT_GIT="$2"; shift 2 ;;
        --base-vm) BASE_VM="$2"; shift 2 ;;
        --base-image) BASE_IMAGE="$2"; shift 2 ;;
        --app) APP="$2"; shift 2 ;;
        --spacesd-source) SPACESD_SOURCE="$2"; shift 2 ;;
        --push) PUSH_TAG="$2"; shift 2 ;;
        --keep) KEEP=1; shift ;;
        --out) OUT="$2"; shift 2 ;;
        --memory) MEMORY="$2"; shift 2 ;;
        -h|--help) sed -n '2,50p' "$0"; exit 0 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
REPOSITORY=ghcr.io/trycua/macos
# shellcheck source=versions.env
. "$HERE/versions.env"
case "$TIER" in
    slim) SUFFIX=-slim PIN_RE="^${VERSION}-slim-[0-9]{8}-[0-9a-f]{7}$" ;;
    full) SUFFIX="" PIN_RE="^${VERSION}-[0-9]{8}-[0-9a-f]{7}$" ;;
    xcode) SUFFIX=-xcode PIN_RE="^${VERSION}-xcode(-[0-9.]+)?-[0-9]{8}-[0-9a-f]{7}$" ;;
    *) echo "--tier is slim, full or xcode" >&2; exit 2 ;;
esac
case "$SPACESD_SOURCE" in
    local) ;;
    release) [ -n "$APP" ] || { echo "--spacesd-source release needs --app (the release's app bundle)" >&2; exit 2; } ;;
    *) echo "--spacesd-source is local or release" >&2; exit 2 ;;
esac
if [ "$TIER" != slim ] && [ -z "$BASE_VM" ]; then
    echo "--tier $TIER builds on the gated VM of the tier below: pass --base-vm" >&2; exit 2
fi
[ -n "$MEMORY" ] || { [ "$TIER" = xcode ] && MEMORY=8GB || MEMORY=4GB; }
MACOS_BASE_IMAGE="$MACOS_BASE_REPO:$MACOS_BASE_TAG"
BASE_IMAGE="${BASE_IMAGE:-$MACOS_BASE_IMAGE}"
VM="${VM:-cua-e2e-macos-$TIER-$$}"
CACHE="${CUA_MACOS_CACHE:-$HOME/.cache/cua-images/macos-cache}"
XCODE_CACHE="${CUA_XCODE_CACHE:-$HOME/XcodesCache}"
REV="$(git -C "$REPO" rev-parse HEAD)"
log() { echo "[macos-build $(date +%T)] $*" >&2; }
guest() { lume ssh "$VM" -t "${2:-120}" "$1" </dev/null; }

command -v lume >/dev/null || { echo "lume is required" >&2; exit 2; }
if [ -n "$PUSH_TAG" ]; then
    # Any tier's pin: ^26(-slim|-xcode(-[0-9.]+)?)?-<yyyymmdd>-<sha7>$, and it
    # must be this tier's.
    [[ "$PUSH_TAG" =~ ^${VERSION}(-slim|-xcode(-[0-9.]+)?)?-[0-9]{8}-[0-9a-f]{7}$ ]] &&
        [[ "$PUSH_TAG" =~ $PIN_RE ]] ||
        { echo "--push takes a new immutable $TIER pin ${VERSION}${SUFFIX}[-X.Y]-<yyyymmdd>-<sha7>" >&2; exit 2; }
    "$REPO/scripts/images/check-tag-safety.sh" "$REPOSITORY:$PUSH_TAG"
    : "${GITHUB_USERNAME:?lume push needs GITHUB_USERNAME}" "${GITHUB_TOKEN:?lume push needs GITHUB_TOKEN}"
fi

# --- pins: image.json's tier claims name the versions.env versions ----------
python3 - "$HERE/image.json" "$HERE/versions.env" <<'PY'
import json, re, sys
claims = json.load(open(sys.argv[1]))
env = dict(l.split("=", 1) for l in open(sys.argv[2]).read().splitlines()
           if l and not l.startswith("#") and "=" in l)
tiers = claims["tiers"]
want = {  # claim -> versions.env key, per tier
    ("slim", "apps", "chrome"): "CHROME_VERSION",
    ("full", "tools", "brew"): "HOMEBREW_TAG", ("full", "tools", "gh"): "GH_VERSION",
    ("full", "tools", "jq"): "JQ_VERSION", ("full", "tools", "rg"): "RG_VERSION",
    ("full", "tools", "uv"): "UV_VERSION", ("full", "tools", "node"): "NODE_VERSION",
    ("full", "tools", "pnpm"): "PNPM_VERSION", ("full", "tools", "go"): "GO_VERSION",
    ("full", "tools", "rustc"): "RUST_VERSION", ("full", "tools", "cargo"): "RUST_VERSION",
    ("xcode", "tools", "xcodebuild"): "XCODE_VERSION",
}
bad = []
for (tier, kind, name), key in want.items():
    section = claims["claims"] if tier == "slim" else tiers[tier]["claims"]
    expect = section[kind][name]["expect"]
    version = env[key].strip('"')
    if not re.search(expect, version) and re.escape(version) not in expect:
        bad.append(f"{tier} {kind}.{name} expect {expect!r} does not name {key}={version}")
runtimes = tiers["xcode"]["claims"]["simulator_runtimes"]
if runtimes != ["iOS " + env["XCODE_IOS_RUNTIME"]]:
    bad.append(f"xcode simulator_runtimes {runtimes} != iOS {env['XCODE_IOS_RUNTIME']}")
if bad:
    sys.exit("image.json and versions.env disagree:\n  " + "\n  ".join(bad))
PY

# --- stage: app bundle, guest files, build identity, a build-time token ------
mkdir -p "$OUT"
SETUP="$OUT/setup"   # shared as /Volumes/My Shared Files/setup (as the SDK does)
rm -rf "$SETUP"; mkdir -p "$SETUP"; chmod 700 "$SETUP"
if [ -z "$APP" ]; then
    [ -z "$(git -C "$REPO" status --porcelain -- libs/cua-spacesd libs/cua-driver libs/cua)" ] ||
        log "warning: uncommitted changes under libs/; build-info git_sha will not describe them"
    log "building cua-spacesd (release) and Cua Spacesd.app"
    (cd "$REPO/libs/cua-spacesd" && cargo build -p cua-spacesd --release)
    bash "$REPO/libs/cua-spacesd/scripts/build-macos-app.sh" >/dev/null
    APP="${CARGO_TARGET_DIR:-$REPO/libs/cua-spacesd/target}/macos/Cua Spacesd.app"
    EXPECT_GIT="${EXPECT_GIT:-$REV}"
fi
ditto "$APP" "$SETUP/Cua Spacesd.app"
printf '%s\n' "$SPACESD_SOURCE" >"$SETUP/spacesd-source"
cp "$HERE"/files/*.sh "$HERE"/files/*.plist "$HERE/image.json" "$HERE/versions.env" "$SETUP/"

# This tier's downloads: fetched once into the cache, checked, then linked
# into the setup share (cache/).
mkdir -p "$CACHE" "$SETUP/cache"
fetch() {  # URL SHA256
    local f; f="$CACHE/$(basename "$1")"
    if [ ! -f "$f" ] || [ "$(shasum -a 256 "$f" | cut -d' ' -f1)" != "$2" ]; then
        log "fetching $1"
        curl -fsSL --retry 3 -o "$f.part" "$1"
        [ "$(shasum -a 256 "$f.part" | cut -d' ' -f1)" = "$2" ] ||
            { rm -f "$f.part"; echo "checksum mismatch: $1" >&2; exit 1; }
        mv "$f.part" "$f"
    fi
    ln -f "$f" "$SETUP/cache/" 2>/dev/null || cp "$f" "$SETUP/cache/"
}
case "$TIER" in
    slim) fetch "$CHROME_URL" "$CHROME_SHA256" ;;
    full)
        fetch "$GH_URL" "$GH_SHA256"; fetch "$JQ_URL" "$JQ_SHA256"; fetch "$RG_URL" "$RG_SHA256"
        fetch "$UV_URL" "$UV_SHA256"; fetch "$NODE_URL" "$NODE_SHA256"; fetch "$PNPM_URL" "$PNPM_SHA256"
        fetch "$GO_URL" "$GO_SHA256"; fetch "$RUSTUP_URL" "$RUSTUP_SHA256" ;;
    xcode)
        xip="$XCODE_CACHE/$XCODE_XIP"
        [ -f "$xip" ] || { echo "$xip is missing: put the Xcode $XCODE_VERSION .xip in CUA_XCODE_CACHE" >&2; exit 2; }
        [ "$(shasum -a 1 "$xip" | cut -d' ' -f1)" = "$XCODE_XIP_SHA1" ] ||
            { echo "checksum mismatch: $xip" >&2; exit 1; }
        ln -f "$xip" "$SETUP/cache/" 2>/dev/null || cp "$xip" "$SETUP/cache/" ;;
esac
"$SETUP/Cua Spacesd.app/Contents/MacOS/cua-spacesd" build-info >"$OUT/build-info.json"
# A prebuilt bundle: the doctor checks the guest runs exactly that build.
EXPECT_GIT="${EXPECT_GIT:-$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1])).get("git_sha",""))' "$OUT/build-info.json")}"
python3 "$REPO/libs/images/common/tools/cua-image-manifest" generate \
    --image-json "$HERE/image.json" --tier "$TIER" --variant lume --arch arm64 --spacesd-source "$SPACESD_SOURCE" \
    --spacesd /usr/local/bin/cua-spacesd --build-info "$OUT/build-info.json" \
    --source-revision "$REV" --ref "$REPOSITORY:$VERSION$SUFFIX" --out "$SETUP/manifest.json"
# The build boots with a token the way the cua SDK delivers one (setup share);
# sanitize-guest.sh removes the guest copy before the push.
(umask 077; openssl rand -hex 32 >"$SETUP/env-token")

cleanup() {
    rm -f "$SETUP/env-token"
    rm -rf "$SETUP/cache"
    lume stop "$VM" >/dev/null 2>&1 || true
    # If `lume serve` went away, `lume stop` cannot reach the VM: end the
    # detached `lume run` this script started (matched by its unique VM name).
    local pid
    for pid in $(pgrep -f "lume run $VM --display none" || true); do kill "$pid" 2>/dev/null || true; done
    [ "$KEEP" = 1 ] || lume delete "$VM" --force >/dev/null 2>&1 || true
}
trap cleanup EXIT

# --- VM ----------------------------------------------------------------------
if [ -n "$BASE_VM" ]; then
    log "cloning $BASE_VM -> $VM"
    lume clone "$BASE_VM" "$VM"
else
    if [ "$BASE_IMAGE" = "$MACOS_BASE_IMAGE" ]; then
        [ "$(crane digest "$BASE_IMAGE")" = "$MACOS_BASE_DIGEST" ] ||
            { echo "$BASE_IMAGE no longer has digest $MACOS_BASE_DIGEST" >&2; exit 1; }
    fi
    log "pulling $BASE_IMAGE -> $VM"
    ref="${BASE_IMAGE#ghcr.io/}"
    lume pull "${ref#*/}" "$VM" --registry ghcr.io --organization "${ref%%/*}"
fi
base_memory="$(lume get "$VM" --format json | python3 -c 'import json,sys;d=json.load(sys.stdin);d=d[0] if isinstance(d,list) else d;print(d["memorySize"])')"
lume set "$VM" --memory "$MEMORY"

boot() {
    lume run "$VM" --display none --detach --shared-dir "$SETUP:ro" --log-file "$OUT/vm.log" >/dev/null
    local up=0
    for _ in $(seq 1 60); do guest 'echo up' 2>/dev/null | grep -q up && { up=1; break; }; sleep 5; done
    [ "$up" = 1 ] || { echo "$VM never answered over ssh" >&2; return 1; }
    # ssh answers before the autologin session is up; AppKit calls (the
    # pasteboard, which `cua-spacesd build-info` touches) need that session.
    for _ in $(seq 1 36); do guest 'pgrep -qx Dock && pgrep -qx Finder' 2>/dev/null && return 0; sleep 5; done
    echo "$VM: the login session never started" >&2; return 1
}
wait_down() {
    for _ in $(seq 1 60); do
        [ "$(lume get "$VM" --format json 2>/dev/null | python3 -c 'import json,sys;d=json.load(sys.stdin);d=d[0] if isinstance(d,list) else d;print(d["status"])' 2>/dev/null)" = stopped ] && return 0
        sleep 5
    done
    return 1
}
log "booting $VM"
boot
guest 'sw_vers -productVersion; csrutil status'
# TCC.db is SIP-protected: the seed (and so the image) needs a SIP-off base.
guest 'csrutil status' | grep -q 'disabled' ||
    { echo "the base VM has SIP enabled; TCC grants cannot be seeded (disable SIP in recoveryOS first)" >&2; exit 1; }

S="/Volumes/My Shared Files/setup"
# A tier above slim keeps the cua-spacesd (and its TCC grants) of the gated
# VM below when it is the same build, and only takes this tier's identity.
want_exe="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["exe_sha256"])' "$OUT/build-info.json")"
have_exe="$(guest '/usr/local/bin/cua-spacesd build-info' 2>/dev/null |
    python3 -c 'import json,sys;print(json.load(sys.stdin).get("exe_sha256",""))' 2>/dev/null || true)"
if [ "$TIER" != slim ] && [ "$have_exe" = "$want_exe" ]; then
    log "cua-spacesd unchanged from the tier below; installing the $TIER identity"
    guest "printf '%s\n' lume | sudo -S -p '' install -o root -g wheel -m 0644 '$S/image.json' '$S/manifest.json' /etc/cua-image/" 60
else
    log "installing cua-spacesd"
    guest "bash '$S/install-guest.sh' '$S'" 900
fi
log "installing the $TIER tier"
guest "bash '$S/tier-$TIER.sh' '$S'" 5400

log "rebooting (the grants and the LaunchAgent must hold across a login)"
booted="$(guest 'sysctl -n kern.boottime')"
guest "printf '%s\n' lume | sudo -S -p '' shutdown -r now" 30 >/dev/null 2>&1 || true
# A guest reboot restarts in place under Virtualization.framework.
for _ in $(seq 1 60); do
    sleep 5
    now="$(guest 'sysctl -n kern.boottime' 15 2>/dev/null || true)"
    [ -n "$now" ] && [ "$now" != "$booted" ] && break
done
[ -n "$now" ] && [ "$now" != "$booted" ] || { echo "$VM did not come back from the reboot" >&2; exit 1; }

# The first login after a tier install indexes everything it added
# (Spotlight on the toolchains): let the load settle (bounded) so the
# accessibility probe's input is not starved.
for _ in $(seq 1 60); do
    load="$(guest 'sysctl -n vm.loadavg' 15 2>/dev/null | awk 'NR==1 {print int($2)}' | tr -dc '0-9')"
    [ -n "$load" ] && [ "$load" -lt 8 ] && break
    sleep 10
done
log "doctor --strict + accessibility/input probe (1-min load ${load:-?})"
expect=()
[ -z "$EXPECT_GIT" ] || expect=(--expect "cua-spacesd=git:$EXPECT_GIT")
"$HERE/doctor-gate.sh" --vm "$VM" --out "$OUT/doctor" --token-file "$SETUP/env-token" ${expect[@]+"${expect[@]}"}

log "sanitizing and shutting down"
guest "bash '$S/sanitize-guest.sh'" 120
guest "printf '%s\n' lume | sudo -S -p '' shutdown -h now" 30 >/dev/null 2>&1 || true
wait_down || { log "guest did not power off; stopping"; lume stop "$VM" >/dev/null; }
lume set "$VM" --memory "$((base_memory / 1024 / 1024))MB"
log "built $VM ($(lume get "$VM" --format json | python3 -c 'import json,sys;d=json.load(sys.stdin);d=d[0] if isinstance(d,list) else d;print(d["memorySize"]//2**20, "MiB,", d["cpuCount"], "cpu")'))"

if [ -n "$PUSH_TAG" ]; then
    log "pushing $REPOSITORY:$PUSH_TAG"
    "$HERE/push.sh" "$VM" "$PUSH_TAG" | tee "$OUT/pushed-digest"
fi

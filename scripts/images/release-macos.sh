#!/usr/bin/env bash
# The macOS half of a cua-spacesd image release, on an Apple silicon Mac with
# Lume (GitHub has no macOS-VM runner for it). One command:
#
#   scripts/images/release-macos.sh X.Y.Z [--dry-run]
#
#   1. download the published cua-spacesd-vX.Y.Z "Cua Spacesd.app" (Developer
#      ID signed and notarized) and check it against the release's SHA256SUMS
#   2. per tier, slim then full (full builds on the gated slim VM):
#      `cua images release libs/images/macos` build and gates (doctor --strict,
#      ax probe, live input), then the keychain check (below), then --publish
#      (push the dated pin, attach the doctor report, verify)
#   3. promote: 26-slim and 26 move to the new pins
#   4. ask CI to refresh the pins PR (cd-images-spacesd-release.yml with
#      build=false): it sees macOS on X.Y.Z, adds it and ticks its box
#
# Keychain check (required, before publishing a tier): a throwaway clone of
# the built VM boots, and its login keychain must unlock with the account
# password `lume`, with no login_renamed_* keychain next to it. It guards the
# Chrome Safe Storage / keychain-password regression (#4445, #4470); the built
# VM itself never boots. The clone, $PREFIX-kc-<tier>, is always deleted.
#
# Rerunning resumes: steps that passed with the same inputs are skipped (the
# stamp is today's date and this checkout's HEAD, so rerun from the same
# commit on the same day, or pass STAMP=...).
#
# Everything lives under $WORK (default ~/.cache/cua-images/macos-release/X.Y.Z):
# the app, the release work and evidence, an isolated CUA_HOME and sandbox
# state, so your own ~/.cua and Spaces are never touched. The only VMs it
# creates or deletes are $PREFIX-slim, $PREFIX-full and their keychain clones
# $PREFIX-kc-{slim,full} (cua-e2e-macos-rel-*).
#
# Disk: a tier needs its VM (~30 GB) plus lume's push cache (~23 GB, inside
# the VM directory), so the run refuses to start below MIN_FREE_GB + 60 GB
# free, deletes each tier's push cache once its pin is verified, and stops if
# free space drops under MIN_FREE_GB (default 200) between steps.
#
# Registry: crane and oras log in to ghcr.io under an isolated DOCKER_CONFIG
# ($WORK/docker), never your ~/.docker. With RECORD_LEDGER=false the doctor
# verdicts are recorded in a local ledger, $WORK/ledger, which
# scripts/images/check-docs-image-refs.py --ledger can read.
#
# Needs: lume (`lume serve`), the cua CLI ($CUA, default `cua` on PATH, or
# build it: cargo build --release -p cua-cli in libs/cua), crane, oras, jq,
# uv, gh (logged in with write:packages, for ghcr.io and the dispatch).
# Knobs: STAMP, PREFIX, WORK, RECORD_LEDGER (default false: trycua/cua has
# no image-doctor-ledger branch yet), CUA, MIN_FREE_GB, LUME_HOME (default
# ~/.lume, where the push caches live).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
VERSION="" DRY=0
for a in "$@"; do
    case "$a" in
        --dry-run) DRY=1 ;;
        -h|--help) sed -n '2,48p' "$0"; exit 0 ;;
        -*) echo "unknown option $a" >&2; exit 2 ;;
        *) VERSION="${a#cua-spacesd-v}"; VERSION="${VERSION#v}" ;;
    esac
done
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "usage: release-macos.sh X.Y.Z [--dry-run]" >&2; exit 2; }
[ "$(uname -s)-$(uname -m)" = Darwin-arm64 ] || [ "$DRY" = 1 ] || { echo "needs an Apple silicon Mac" >&2; exit 2; }
REPO_SLUG=trycua/cua
TAG="cua-spacesd-v$VERSION"
ASSET=cua-spacesd-macos-universal.app.zip
CUA="${CUA:-cua}"
STAMP="${STAMP:-$(date -u +%Y%m%d)-$(git -C "$ROOT" rev-parse --short=7 HEAD)}"
PREFIX="${PREFIX:-cua-e2e-macos-rel-${STAMP%%-*}}"
WORK="${WORK:-${CUA_IMAGES_OUT:-$HOME/.cache/cua-images}/macos-release/$VERSION}"
RECORD_LEDGER="${RECORD_LEDGER:-false}"
MIN_FREE_GB="${MIN_FREE_GB:-200}"
LUME_HOME="${LUME_HOME:-$HOME/.lume}"
log() { echo "[release-macos $(date +%T)] $*" >&2; }
run() { log "+ $*"; [ "$DRY" = 1 ] || "$@"; }

# The images must carry exactly the released daemon.
[ "$(gh release view "$TAG" -R "$REPO_SLUG" --json isDraft,isPrerelease -q '.isDraft or .isPrerelease')" = false ] ||
    { echo "$TAG is not a published release" >&2; exit 1; }
for tool in lume "$CUA" crane oras jq uv gh; do
    command -v "$tool" >/dev/null || [ "$DRY" = 1 ] || { echo "missing $tool" >&2; exit 2; }
done

free_gb() { df -g "$LUME_HOME" | awk 'NR==2 {print $4}'; }
need_free() {  # GB: stop (resumable) rather than fill the disk
    [ "$DRY" = 1 ] && return 0
    local f; f="$(free_gb)"
    [ "$f" -ge "$1" ] || { echo "only ${f} GB free under $LUME_HOME (need ${1} GB); free some space and rerun (it resumes)" >&2; exit 1; }
}
# A tier's push cache (lume's compressed chunks, ~23 GB) inside its VM
# directory: not needed once the pin is verified, and the full tier would
# clone slim's.
drop_push_cache() { run rm -rf "$LUME_HOME/$PREFIX-$1/.lume_oci_push_cache"; }
need_free $((MIN_FREE_GB + 60))

mkdir -p "$WORK/app" "$WORK/state" "$WORK/cuahome" "$WORK/docker"
if [ "$DRY" = 1 ]; then
    log "would download $TAG $ASSET into $WORK/app"
elif [ ! -d "$WORK/app/Cua Spacesd.app" ]; then
    log "downloading $TAG $ASSET"
    rm -f "$WORK/$ASSET" "$WORK/SHA256SUMS"
    gh release download "$TAG" -R "$REPO_SLUG" -D "$WORK" -p "$ASSET" -p SHA256SUMS
    (cd "$WORK" && grep " $ASSET\$" SHA256SUMS | shasum -a 256 -c -)
    ditto -x -k "$WORK/$ASSET" "$WORK/app"
fi
[ "$DRY" = 1 ] || [ -d "$WORK/app/Cua Spacesd.app" ] || { echo "$ASSET has no Cua Spacesd.app" >&2; exit 1; }

export CUA_HOME="$WORK/cuahome" CUA_TELEMETRY=0 DO_NOT_TRACK=1
export CUA_MACOS_APP="$WORK/app/Cua Spacesd.app" CUA_MACOS_SPACESD_SOURCE=release
if [ "$DRY" = 0 ]; then
    export GITHUB_USERNAME="${GITHUB_USERNAME:-$(gh api user -q .login)}"
    export GITHUB_TOKEN="${GITHUB_TOKEN:-$(gh auth token)}"
fi
# annotate.sh, the doctor attach and verify push and read with crane/oras:
# log them in under the run's own DOCKER_CONFIG, never ~/.docker.
export DOCKER_CONFIG="$WORK/docker"
if [ "$DRY" = 0 ]; then
    printf '%s' "$GITHUB_TOKEN" | crane auth login ghcr.io -u "$GITHUB_USERNAME" --password-stdin >/dev/null
    printf '%s' "$GITHUB_TOKEN" | oras login ghcr.io -u "$GITHUB_USERNAME" --password-stdin >/dev/null
fi
common=(--stamp "$STAMP" --var "prefix=$PREFIX" --var "record_ledger=$RECORD_LEDGER" --embedded --state-dir "$WORK/state")
log "cua-spacesd $VERSION, stamp $STAMP, VMs $PREFIX-{slim,full}, work $WORK"

# The login keychain of a throwaway clone of VM unlocks with `lume`, and no
# login_renamed_* keychain exists (see the header). Fails the run otherwise.
keychain_check() (
    vm="$1" out="$2"
    clone="${vm%-*}-kc-${vm##*-}"  # <prefix>-kc-<tier>
    if [ "$DRY" = 1 ]; then log "+ keychain check of $vm on a clone $clone"; exit 0; fi
    kc_cleanup() {
        lume stop "$clone" >/dev/null 2>&1 || true
        lume delete "$clone" --force >/dev/null 2>&1 || true
    }
    trap kc_cleanup EXIT  # this subshell's exit: the clone never outlives the check
    kc_cleanup
    lume clone "$vm" "$clone" >/dev/null
    lume set "$clone" --memory 4096MB >/dev/null
    lume run "$clone" --display none --detach >/dev/null
    g() { lume ssh "$clone" -t "${2:-60}" "$1" </dev/null; }
    for _ in $(seq 1 60); do g 'echo up' 2>/dev/null | grep -q up && break; sleep 5; done
    for _ in $(seq 1 60); do g 'pgrep -qx Dock && pgrep -qx Finder' 2>/dev/null && break; sleep 5; done
    # shellcheck disable=SC2016 # expanded in the guest
    g 'kc="$HOME/Library/Keychains/login.keychain-db"
       ls "$HOME/Library/Keychains" | tr "\n" " "; echo
       security lock-keychain "$kc"
       security unlock-keychain -p lume "$kc" && echo UNLOCK_OK || echo UNLOCK_FAIL
       echo "RENAMED=$(ls "$HOME/Library/Keychains" | grep -c login_renamed)"' 120 >"$out" 2>&1 || true
    cat "$out" >&2
    grep -q '^UNLOCK_OK' "$out" || { echo "keychain check: $vm's login keychain does not unlock with lume ($out)" >&2; exit 1; }
    grep -q '^RENAMED=0$' "$out" || { echo "keychain check: $vm has login_renamed keychains ($out)" >&2; exit 1; }
    log "keychain check: $vm unlocks with lume, no login_renamed keychains"
)

# RECORD_LEDGER=false: the pin's doctor verdict still goes into a local
# ledger ($WORK/ledger) for check-docs-image-refs.py --ledger.
record_local_ledger() {
    [ "$RECORD_LEDGER" = true ] && return 0
    local e="$WORK/$1/evidence"
    if [ "$DRY" = 1 ]; then log "+ record $1 in $WORK/ledger"; return 0; fi
    python3 "$ROOT/scripts/images/doctor_ledger.py" record --ledger "$WORK/ledger" --repo ghcr.io/trycua/macos \
        --digest "$(jq -r .lume "$e/pushed.json")" --report "$e/doctor/arm64-lume/report.json" \
        --lane "$e/doctor/arm64-lume/lane.json" --report-ref "$(jq -r '.[0].report_ref' "$e/attested.json")" \
        --run "local:$STAMP (release-macos.sh $VERSION)" >/dev/null
    log "$1: doctor verdict recorded in $WORK/ledger"
}

cd "$ROOT"
for tier in slim full; do
    need_free $((MIN_FREE_GB + 35))
    log "$tier: build and gates (evidence: $WORK/$tier/evidence)"
    run "$CUA" images release libs/images/macos --tier "$tier" "${common[@]}" --work "$WORK/$tier" --resume
    log "$tier: keychain check on a throwaway clone"
    mkdir -p "$WORK/$tier"
    keychain_check "$PREFIX-$tier" "$WORK/$tier/keychain.txt"
    log "$tier: push, attach the doctor report, verify"
    need_free $((MIN_FREE_GB + 25))
    run "$CUA" images release libs/images/macos --tier "$tier" "${common[@]}" --work "$WORK/$tier" --resume --publish
    drop_push_cache "$tier"
    record_local_ledger "$tier"
    # The full tier is built; the slim VM it cloned is no longer needed.
    [ "$tier" = slim ] || run lume delete "$PREFIX-slim" --force
done
for tier in slim full; do
    log "$tier: promote"
    run "$CUA" images release libs/images/macos --tier "$tier" "${common[@]}" --work "$WORK/$tier" --resume --publish --promote --steps promote
done
run lume delete "$PREFIX-full" --force

log "macOS is on cua-spacesd $VERSION; refreshing the pins PR in CI"
run gh workflow run cd-images-spacesd-release.yml -R "$REPO_SLUG" -f "version=$VERSION" -f build=false
log "follow it: gh run list -R $REPO_SLUG -w cd-images-spacesd-release.yml -L 1"

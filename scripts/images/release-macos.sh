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
#      ax probe, live input), then --publish (push the dated pin, attach the
#      doctor report, verify)
#   3. promote: 26-slim and 26 move to the new pins
#   4. ask CI to refresh the pins PR (cd-images-spacesd-release.yml with
#      build=false): it sees macOS on X.Y.Z, adds it and ticks its box
#
# Rerunning resumes: steps that passed with the same inputs are skipped (the
# stamp is today's date and this checkout's HEAD, so rerun from the same
# commit on the same day, or pass STAMP=...).
#
# Everything lives under $WORK (default ~/.cache/cua-images/macos-release/X.Y.Z):
# the app, the release work and evidence, an isolated CUA_HOME and sandbox
# state, so your own ~/.cua and Spaces are never touched. The only VMs it
# creates or deletes are $PREFIX-slim and $PREFIX-full (cua-e2e-macos-rel-*).
#
# Needs: lume (`lume serve`), the cua CLI ($CUA, default `cua` on PATH, or
# build it: cargo build --release -p cua-cli in libs/cua), crane, oras, jq,
# uv, gh (logged in with write:packages, for ghcr.io and the dispatch).
# Knobs: STAMP, PREFIX, WORK, RECORD_LEDGER (default false: the doctor
# ledger branch is CI's), CUA.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
VERSION="" DRY=0
for a in "$@"; do
    case "$a" in
        --dry-run) DRY=1 ;;
        -h|--help) sed -n '2,29p' "$0"; exit 0 ;;
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
log() { echo "[release-macos $(date +%T)] $*" >&2; }
run() { log "+ $*"; [ "$DRY" = 1 ] || "$@"; }

# The images must carry exactly the released daemon.
[ "$(gh release view "$TAG" -R "$REPO_SLUG" --json isDraft,isPrerelease -q '.isDraft or .isPrerelease')" = false ] ||
    { echo "$TAG is not a published release" >&2; exit 1; }
for tool in lume "$CUA" crane oras jq uv gh; do
    command -v "$tool" >/dev/null || [ "$DRY" = 1 ] || { echo "missing $tool" >&2; exit 2; }
done

mkdir -p "$WORK/app" "$WORK/state" "$WORK/cuahome"
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
common=(--stamp "$STAMP" --var "prefix=$PREFIX" --var "record_ledger=$RECORD_LEDGER" --embedded --state-dir "$WORK/state")
log "cua-spacesd $VERSION, stamp $STAMP, VMs $PREFIX-{slim,full}, work $WORK"

cd "$ROOT"
for tier in slim full; do
    log "$tier: build and gates (evidence: $WORK/$tier/evidence)"
    run "$CUA" images release libs/images/macos --tier "$tier" "${common[@]}" --work "$WORK/$tier" --resume
    log "$tier: push, attach the doctor report, verify"
    run "$CUA" images release libs/images/macos --tier "$tier" "${common[@]}" --work "$WORK/$tier" --resume --publish
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

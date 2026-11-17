#!/usr/bin/env bash
# Publish a Windows primary, the index `Image.windows()` and the `windows`
# CLI alias resolve (`ghcr.io/trycua/windows:2022`), for a containerDisk:
#
#   scripts/images/publish-windows-primary.sh --disk REF --spacesd true|false
#       [--stamp YYYYMMDD-sha7] [--dry-run] [--promote] [--log FILE]
#
# Windows ships only as a KubeVirt containerDisk (`2022-disk*`). The primary
# is a copy of that index (same child manifests, so the same content) whose
# annotations link the variant, as annotate-canonical.sh does for Linux:
#   ai.cua.image.os        windows
#   ai.cua.spacesd         what the disk runs (true for the
#                          libs/images/windows-2022 build)
#   ai.cua.env-driver      same value (legacy label, for SDKs before the rename)
#   ai.cua.image.variants  {"containerdisk":"ghcr.io/trycua/windows@<disk digest>"}
# It is pushed as the NEW immutable tag `2022-<stamp>` (refused if it exists,
# check-tag-safety.sh) and its digest printed. With --promote the moving tag
# `2022` is re-pointed to it too; do that only after `cua-spacesd doctor
# --strict` passed against the disk's exact digest (cd-image-windows.yml).
#
# Needs crane, oras and jq, and push access to ghcr.io/trycua/windows
# (DOCKER_CONFIG). --dry-run builds the document and prints the plan.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHECK="$HERE/check-tag-safety.sh"
DRY=0 PROMOTE=0 DISK="" SPACESD=""
STAMP="$(date -u +%Y%m%d)-$(git -C "$HERE" rev-parse --short=7 HEAD)"
LOG=""
while [ $# -gt 0 ]; do
    case "$1" in
        --disk) DISK="$2"; shift 2 ;;
        --spacesd) SPACESD="$2"; shift 2 ;;
        --dry-run) DRY=1; shift ;;
        --promote) PROMOTE=1; shift ;;
        --stamp) STAMP="$2"; shift 2 ;;
        --log) LOG="$2"; shift 2 ;;
        -h|--help) sed -n '2,23p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
[ -n "$DISK" ] || { echo "--disk is required" >&2; exit 2; }
[[ "$SPACESD" =~ ^(true|false)$ ]] || { echo "--spacesd must be true or false" >&2; exit 2; }
[[ "$STAMP" =~ ^[0-9]{8}-[0-9a-f]{7}$ ]] || { echo "bad stamp $STAMP" >&2; exit 2; }
for t in crane oras jq; do command -v "$t" >/dev/null || { echo "missing $t" >&2; exit 2; }; done
ORAS_CFG=()
[ -n "${DOCKER_CONFIG:-}" ] && ORAS_CFG=(--registry-config "$DOCKER_CONFIG/config.json")

REPO=ghcr.io/trycua/windows
PRIMARY="$REPO:2022" PIN="$REPO:2022-$STAMP"
INDEX_MT=application/vnd.oci.image.index.v1+json
WORK="$(mktemp -d "${TMPDIR:-/tmp}/cua-winprimary.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
log() { echo "$*"; [ -z "$LOG" ] || echo "$*" >>"$LOG"; }
sha() { printf 'sha256:%s' "$(shasum -a 256 "$1" | cut -d' ' -f1)"; }

DISK_DIGEST="$(crane digest "$DISK")"
crane manifest "$REPO@$DISK_DIGEST" >"$WORK/disk.json"
[ "$(jq -r .mediaType "$WORK/disk.json")" = "$INDEX_MT" ] || { echo "$DISK is not an OCI index" >&2; exit 1; }
jq --arg variants "{\"containerdisk\":\"$REPO@$DISK_DIGEST\"}" --arg s "$SPACESD" \
    '.annotations = ((.annotations // {}) + {"ai.cua.image.os": "windows", "ai.cua.spacesd": $s, "ai.cua.env-driver": $s, "ai.cua.image.variants": $variants})
     | del(.annotations["ai.cua.image.variant"])' \
    "$WORK/disk.json" >"$WORK/primary.json"
# Same children as the disk: only annotations differ.
[ "$(jq -c .manifests "$WORK/disk.json")" = "$(jq -c .manifests "$WORK/primary.json")" ] || exit 1
WANT="$(sha "$WORK/primary.json")"

# The pin is new (or already this exact document).
"$CHECK" --digest "$WANT" "$PIN"

log "stamp $STAMP (dry-run=$DRY, promote=$PROMOTE)"
log "- disk \`$DISK\`: \`$DISK_DIGEST\`"
log "- \`$PIN\` (new): \`$WANT\`"
if [ "$DRY" = 1 ]; then jq . "$WORK/primary.json"; log "dry run: nothing pushed"; exit 0; fi

if [ "$(crane digest "$PIN" 2>/dev/null || true)" != "$WANT" ]; then
    oras manifest push ${ORAS_CFG[@]+"${ORAS_CFG[@]}"} --media-type "$INDEX_MT" "$PIN" "$WORK/primary.json" >/dev/null
fi
[ "$(crane digest "$PIN")" = "$WANT" ] || { echo "$PIN is not $WANT after the push" >&2; exit 1; }
log "pushed \`$PIN\` at \`$WANT\`"
if [ "$PROMOTE" = 1 ]; then
    "$CHECK" --moving --digest "$WANT" "$PRIMARY"
    crane tag "$REPO@$WANT" 2022
    [ "$(crane digest "$PRIMARY")" = "$WANT" ] || { echo "$PRIMARY is not $WANT after tagging" >&2; exit 1; }
    log "moved \`$PRIMARY\` to \`$WANT\`"
fi
echo "primary_digest=$WANT"

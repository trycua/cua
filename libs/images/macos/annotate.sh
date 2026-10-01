#!/usr/bin/env bash
# Publish the cua metadata on a `lume push`ed macOS image, without touching
# its content: the manifest is re-pushed under a NEW immutable pin with the
# annotations the resolver reads (lume push writes only org.trycua.lume.*).
# Same config and layers, so `lume pull` of either tag gets the same VM.
#
#   libs/images/macos/annotate.sh SOURCE_TAG NEW_TAG [--dry-run]
#       [--tier slim|full|xcode] [--content-key sha256:...]
#     e.g. annotate.sh 26-20260925-56503f3 26-20260925-<sha7>
#
# Adds: ai.cua.image.os=macos, ai.cua.spacesd=true, ai.cua.env-driver=true
# (pre-rename key), ai.cua.image.variant=lume, ai.cua.image.variants
# {"lume": "<repo>:NEW_TAG"}, ai.cua.image.source (<repo>:SOURCE_TAG@digest),
# org.opencontainers.image.source, and with the options ai.cua.image.tier and
# ai.cua.image.content-key (tools/content-key.py; a scheduled rebuild moves
# a floating tag only when its key differs). Move the floating tag afterwards with
# `crane tag <repo>@<printed digest> 26`, once the strict doctor passed on
# NEW_TAG. Needs crane, oras, jq and push access (DOCKER_CONFIG).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPOSITORY=ghcr.io/trycua/macos
SRC="${1:?source tag}" NEW="${2:?new tag}" DRY=0 TIER="" KEY=""
shift 2
while [ $# -gt 0 ]; do
    case "$1" in
        --dry-run) DRY=1; shift ;;
        --tier) TIER="$2"; shift 2 ;;
        --content-key) KEY="$2"; shift 2 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
MT=application/vnd.oci.image.manifest.v1+json
WORK="$(mktemp -d "${TMPDIR:-/tmp}/cua-macos-annotate.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

"$HERE/../../../scripts/images/check-tag-safety.sh" "$REPOSITORY:$NEW" >&2
src_digest="$(crane digest "$REPOSITORY:$SRC")"
crane manifest "$REPOSITORY@$src_digest" >"$WORK/src.json"
[ "$(jq -r .mediaType "$WORK/src.json")" = "$MT" ] || { echo "$SRC is not an OCI manifest" >&2; exit 1; }
jq --arg variants "{\"lume\":\"$REPOSITORY:$NEW\"}" --arg source "$REPOSITORY:$SRC@$src_digest" \
    --arg tier "$TIER" --arg key "$KEY" \
    '.annotations = ((.annotations // {}) + {
        "ai.cua.image.os": "macos",
        "ai.cua.spacesd": "true",
        "ai.cua.env-driver": "true",
        "ai.cua.image.variant": "lume",
        "ai.cua.image.variants": $variants,
        "ai.cua.image.source": $source,
        "org.opencontainers.image.source": "https://github.com/trycua/cua"}
        + (if $tier != "" then {"ai.cua.image.tier": $tier} else {} end)
        + (if $key != "" then {"ai.cua.image.content-key": $key} else {} end))' \
    "$WORK/src.json" >"$WORK/new.json"
# Content is unchanged: same config and layers.
[ "$(jq -c '[.config, .layers]' "$WORK/src.json")" = "$(jq -c '[.config, .layers]' "$WORK/new.json")" ] || exit 1
want="sha256:$(shasum -a 256 "$WORK/new.json" | cut -d' ' -f1)"
echo "source $REPOSITORY:$SRC $src_digest" >&2
if [ "$DRY" = 1 ]; then echo "$want (dry run)"; exit 0; fi
cfg=()
[ -n "${DOCKER_CONFIG:-}" ] && cfg=(--registry-config "$DOCKER_CONFIG/config.json")
oras manifest push ${cfg[@]+"${cfg[@]}"} --media-type "$MT" "$REPOSITORY:$NEW" "$WORK/new.json" >/dev/null
got="$(crane digest "$REPOSITORY:$NEW")"
[ "$got" = "$want" ] || { echo "pushed $NEW but it is $got, not $want" >&2; exit 1; }
echo "$got"

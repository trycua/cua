#!/usr/bin/env bash
# Re-publish the canonical images with their cua metadata, without rebuilding:
#
#   scripts/images/annotate-canonical.sh [--dry-run] [--stamp YYYYMMDD-sha7] [--log FILE]
#
# For each canonical tag (below) the current document is read and pushed
# again under a NEW immutable tag `<tag>-<stamp>` with:
#   ai.cua.image.os        linux | windows | macos
#   ai.cua.spacesd          true | false (what the image actually runs)
#   ai.cua.env-driver      same value (legacy label, for SDKs before the rename)
#   ai.cua.image.variants  JSON map variant -> pinned ref (every primary)
# as index/manifest annotations, and as config labels on the Linux images'
# per-arch manifests (their configs are rewritten; layers are untouched, so
# the content is identical apart from metadata). Windows keeps its child
# manifest as is (its provenance attestation references it), macOS (Lume)
# manifests get annotations only. Then the moving tag is re-pointed at the
# new digest. Existing immutable tags are never touched; every push is
# guarded by check-tag-safety.sh.
#
# Needs crane, oras and jq, and push access to ghcr.io/trycua/{linux,windows,macos}
# (DOCKER_CONFIG; oras reads $DOCKER_CONFIG/config.json). --dry-run builds
# every document and prints the plan without pushing.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHECK="$HERE/check-tag-safety.sh"
DRY=0
STAMP="$(date -u +%Y%m%d)-$(git -C "$HERE" rev-parse --short=7 HEAD)"
LOG=""
while [ $# -gt 0 ]; do
    case "$1" in
        --dry-run) DRY=1; shift ;;
        --stamp) STAMP="$2"; shift 2 ;;
        --log) LOG="$2"; shift 2 ;;
        -h|--help) sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
[[ "$STAMP" =~ ^[0-9]{8}-[0-9a-f]{7}$ ]] || { echo "bad stamp $STAMP" >&2; exit 2; }
for t in crane oras jq; do command -v "$t" >/dev/null || { echo "missing $t" >&2; exit 2; }; done
ORAS_CFG=()
[ -n "${DOCKER_CONFIG:-}" ] && ORAS_CFG=(--registry-config "$DOCKER_CONFIG/config.json")

WORK="$(mktemp -d "${TMPDIR:-/tmp}/cua-annotate.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
INDEX_MT=application/vnd.oci.image.index.v1+json
MANIFEST_MT=application/vnd.oci.image.manifest.v1+json

log() { echo "$*"; [ -z "$LOG" ] || echo "$*" >>"$LOG"; }
sha() { printf 'sha256:%s' "$(shasum -a 256 "$1" | cut -d' ' -f1)"; }
size() { wc -c <"$1" | tr -d ' '; }

# push_manifest REPO[:TAG] FILE MEDIATYPE -> prints the digest
push_manifest() {
    set -e  # command substitutions do not inherit errexit in bash 3.2
    local ref="$1" file="$2" mt="$3"
    if [ "$DRY" = 1 ]; then sha "$file"; return; fi
    oras manifest push ${ORAS_CFG[@]+"${ORAS_CFG[@]}"} --media-type "$mt" "$ref" "$file" >/dev/null
    local want; want="$(sha "$file")"
    if [[ "$ref" == *:* && "${ref##*/}" == *:* ]]; then
        local got; got="$(crane digest "$ref")"
        [ "$got" = "$want" ] || { echo "pushed $ref but it is $got, not $want" >&2; exit 1; }
    fi
    echo "$want"
}

# relabel REPO CHILD_DIGEST OS ENV -> prints "digest size" of a manifest whose
# config carries the labels (same layers).
relabel() {
    set -e  # command substitutions do not inherit errexit in bash 3.2
    local repo="$1" child="$2" os="$3" env="$4" d="$WORK/${2#sha256:}"
    mkdir -p "$d"
    crane manifest "$repo@$child" >"$d/manifest.json"
    [ "$(jq -r .mediaType "$d/manifest.json")" = "$MANIFEST_MT" ] \
        || { echo "$repo@$child is not an OCI manifest" >&2; exit 1; }
    crane config "$repo@$child" >"$d/config.orig.json"
    jq -c --arg os "$os" --arg env "$env" \
        '.config.Labels = ((.config.Labels // {}) + {"ai.cua.image.os": $os, "ai.cua.spacesd": $env, "ai.cua.env-driver": $env})' \
        "$d/config.orig.json" >"$d/config.json"
    # Layers and rootfs.diff_ids are unchanged.
    [ "$(jq -c '.rootfs' "$d/config.orig.json")" = "$(jq -c '.rootfs' "$d/config.json")" ] || exit 1
    local cdig csize
    cdig="$(sha "$d/config.json")" csize="$(size "$d/config.json")"
    if [ "$DRY" = 0 ]; then
        oras blob push ${ORAS_CFG[@]+"${ORAS_CFG[@]}"} --media-type "$(jq -r .config.mediaType "$d/manifest.json")" \
            "$repo@$cdig" "$d/config.json" >/dev/null
    fi
    jq -c --arg dig "$cdig" --argjson size "$csize" '.config.digest = $dig | .config.size = $size' \
        "$d/manifest.json" >"$d/manifest.new.json"
    [ "$(jq -c .layers "$d/manifest.json")" = "$(jq -c .layers "$d/manifest.new.json")" ] || exit 1
    local mdig; mdig="$(push_manifest "$repo" "$d/manifest.new.json" "$MANIFEST_MT")"
    echo "$mdig $(size "$d/manifest.new.json")"
}

# annotate_index REPO TAG OS ENV VARIANTS_JSON RELABEL(0|1) -> prints the new digest
annotate_index() {
    set -e  # command substitutions do not inherit errexit in bash 3.2
    local repo="$1" tag="$2" os="$3" env="$4" variants="$5" relabel_children="$6"
    local f="$WORK/index-${tag}.json" out="$WORK/index-${tag}.new.json"
    crane manifest "$repo:$tag" >"$f"
    [ "$(jq -r .mediaType "$f")" = "$INDEX_MT" ] || { echo "$repo:$tag is not an OCI index" >&2; exit 1; }
    cp "$f" "$out"
    if [ "$relabel_children" = 1 ]; then
        local n i child new
        n="$(jq '.manifests | length' "$f")"
        for ((i = 0; i < n; i++)); do
            child="$(jq -r ".manifests[$i].digest" "$f")"
            new="$(relabel "$repo" "$child" "$os" "$env")"
            jq --argjson i "$i" --arg dig "${new% *}" --argjson size "${new#* }" \
                '.manifests[$i].digest = $dig | .manifests[$i].size = $size' "$out" >"$out.tmp"
            mv "$out.tmp" "$out"
        done
    fi
    jq --arg os "$os" --arg env "$env" --arg variants "$variants" \
        '.annotations = ((.annotations // {}) + {"ai.cua.image.os": $os, "ai.cua.spacesd": $env, "ai.cua.env-driver": $env, "ai.cua.image.variants": $variants})' \
        "$out" >"$out.tmp"
    mv "$out.tmp" "$out"
    push_manifest "$repo:$tag-$STAMP" "$out" "$INDEX_MT"
}

# annotate_manifest REPO TAG OS ENV VARIANTS_JSON -> prints the new digest (Lume)
annotate_manifest() {
    set -e  # command substitutions do not inherit errexit in bash 3.2
    local repo="$1" tag="$2" os="$3" env="$4" variants="$5"
    local f="$WORK/manifest-${tag}.json" out="$WORK/manifest-${tag}.new.json"
    crane manifest "$repo:$tag" >"$f"
    [ "$(jq -r .mediaType "$f")" = "$MANIFEST_MT" ] || { echo "$repo:$tag is not an OCI manifest" >&2; exit 1; }
    jq --arg os "$os" --arg env "$env" --arg variants "$variants" \
        '.annotations = ((.annotations // {}) + {"ai.cua.image.os": $os, "ai.cua.spacesd": $env, "ai.cua.env-driver": $env, "ai.cua.image.variants": $variants})' \
        "$f" >"$out"
    [ "$(jq -c '[.config, .layers]' "$f")" = "$(jq -c '[.config, .layers]' "$out")" ] || exit 1
    push_manifest "$repo:$tag-$STAMP" "$out" "$MANIFEST_MT"
}

LINUX=ghcr.io/trycua/linux WINDOWS=ghcr.io/trycua/windows MACOS=ghcr.io/trycua/macos
# Indexed arrays (macOS ships bash 3.2): TAGS[i], OLD[i], NEW[i].
TAGS=("$LINUX:24.04-disk" "$LINUX:24.04" "$WINDOWS:2022-disk" "$MACOS:26" "$MACOS:15")
OLD=() NEW=()
for i in "${!TAGS[@]}"; do OLD[i]="$(crane digest "${TAGS[i]}")"; done
# New immutable tags must not exist yet.
pins=()
for t in "${TAGS[@]}"; do pins+=("$t-$STAMP"); done
"$CHECK" "${pins[@]}"

log "stamp $STAMP (dry-run=$DRY)"
# Linux: the disk first; it points back at the rootfs by its (new) immutable tag.
NEW[0]="$(annotate_index "$LINUX" 24.04-disk linux true \
    "{\"rootfs\":\"$LINUX:24.04-$STAMP\"}" 1)"
NEW[1]="$(annotate_index "$LINUX" 24.04 linux true \
    "{\"containerdisk\":\"$LINUX@${NEW[0]}\"}" 1)"
# Windows: computer-server + cua-driver, no cua-spacesd.
NEW[2]="$(annotate_index "$WINDOWS" 2022-disk windows false \
    "{\"containerdisk\":\"$WINDOWS:2022-disk-$STAMP\"}" 0)"
# macOS: 26 is built by libs/images/macos (cua-spacesd at login); 15 is a Lume
# copy of macos-sequoia-cua (computer-server era, no cua-spacesd).
NEW[3]="$(annotate_manifest "$MACOS" 26 macos true "{\"lume\":\"$MACOS:26-$STAMP\"}")"
NEW[4]="$(annotate_manifest "$MACOS" 15 macos false "{\"lume\":\"$MACOS:15-$STAMP\"}")"

for i in "${!TAGS[@]}"; do
    t="${TAGS[i]}"
    log "- \`$t\`: \`${OLD[i]}\` -> \`${NEW[i]}\` (pin \`${t##*:}-$STAMP\`)"
done
[ "$DRY" = 1 ] && { log "dry run: nothing pushed"; exit 0; }

# Re-point the moving tags (canonical repos only).
"$CHECK" --moving "${TAGS[@]}"
for i in "${!TAGS[@]}"; do
    t="${TAGS[i]}"
    crane tag "${t%:*}@${NEW[i]}" "${t##*:}"
    got="$(crane digest "$t")"
    [ "$got" = "${NEW[i]}" ] || { echo "$t is $got after re-tagging, want ${NEW[i]}" >&2; exit 1; }
done
log "moving tags re-pointed"

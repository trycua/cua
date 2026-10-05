#!/usr/bin/env bash
# `cua images release` publish step: immutable dated pins from the pushed
# children, annotated with the content digest and, when doctor-attest ran,
# each child's doctor verdict.
#
#   release-publish.sh --image-dir DIR [--repo REPO] --tag TAG --series SERIES --stamp STAMP --arches a,b
#       --evidence DIR --cua CUA --run-url URL [--tier TIER]
#
# The content digest is the sha256 of the per-arch content digests
# (EVIDENCE/content-digest-<arch>) in arch order, the value scheduled runs
# compare with the promoted tag. Writes EVIDENCE/pins.json (cua images promote
# takes it).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IMAGE_DIR="" REPO="" TAG="" SERIES="" STAMP="" ARCHES="" EVIDENCE="" CUA="cua" RUN_URL="" TIER=""
while [ $# -gt 0 ]; do
    case "$1" in
        --image-dir) IMAGE_DIR="$2"; shift 2 ;;
        --repo) REPO="$2"; shift 2 ;;
        --tag) TAG="$2"; shift 2 ;;
        --series) SERIES="$2"; shift 2 ;;
        --stamp) STAMP="$2"; shift 2 ;;
        --arches) ARCHES="$2"; shift 2 ;;
        --evidence) EVIDENCE="$2"; shift 2 ;;
        --cua) CUA="$2"; shift 2 ;;
        --run-url) RUN_URL="$2"; shift 2 ;;
        --tier) TIER="$2"; shift 2 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
digests=()
IFS=, read -r -a arches <<<"$ARCHES"
[ "${#arches[@]}" -gt 0 ] || { echo "--arches is required" >&2; exit 2; }
for a in "${arches[@]}"; do
    f="$EVIDENCE/content-digest-$a"
    [ -s "$f" ] || { echo "missing $f (the content step of $a)" >&2; exit 1; }
    digests+=("$f")
done
if [ "${#arches[@]}" = 1 ]; then
    content="$(cat "${digests[0]}")"
else
    content="sha256:$(cat "${digests[@]}" | sha256sum | cut -d' ' -f1)"
fi
args=(--annotation ai.cua.doctor.status=pass --annotation "ai.cua.doctor.report=$RUN_URL"
      --annotation "ai.cua.image.content-digest=$content")
if [ -n "$TIER" ]; then args+=(--annotation "ai.cua.image.tier=$TIER"); fi
if [ -f "$EVIDENCE/attested.json" ]; then
    while IFS= read -r a; do args+=("$a"); done < <(python3 "$HERE/attest-doctor-reports.py" descriptor-annotations "$EVIDENCE/attested.json")
fi
repo=(); if [ -n "$REPO" ]; then repo=(--repo "$REPO"); fi
"$CUA" images publish "$IMAGE_DIR" ${repo[@]+"${repo[@]}"} --tag "$TAG" --series "$SERIES" --stamp "$STAMP" \
    --platform "$(printf 'linux/%s,' "${arches[@]}" | sed 's/,$//')" "${args[@]}" --record "$EVIDENCE/pins.json"
cat "$EVIDENCE/pins.json"

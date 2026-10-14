#!/usr/bin/env bash
# What an image build contains, as one digest: scheduled rebuilds publish a
# new pin and move the floating tag only when this changed.
#
#   scripts/images/content-digest.sh [--rootfs REF | --packages FILE] [--tree PATH]... [--out FILE]
#   scripts/images/content-digest.sh --promoted REF
#
# The digest covers:
#   - the installed package list (sorted `name=version`), read from the built
#     rootfs REF (dpkg on Ubuntu, pacman on Arch) or taken from FILE;
#   - the git tree hash of each --tree PATH at HEAD (the image definition and
#     the sources baked into it, e.g. libs/cua-spacesd), so a code change is a
#     content change even when no package moved.
# It prints `sha256:<hex>`; --out also writes the hashed inputs to FILE (the
# evidence for "changed" or "unchanged").
#
# --promoted REF prints the `ai.cua.image.content-digest` annotation of the
# index REF points at (empty when REF or the annotation is missing), which the
# publish jobs write on every pin.
#
# Needs docker (--rootfs), git (--tree) and crane (--promoted; CRANE=... to
# override). Exit codes: 0 ok, 2 usage or lookup error.
set -euo pipefail

usage() { sed -n '2,22p' "$0" | sed 's/^# \{0,1\}//' >&2; exit 2; }

rootfs="" packages="" out="" promoted="" trees=()
while [ $# -gt 0 ]; do
    case "$1" in
        --rootfs) rootfs="${2:?}"; shift 2 ;;
        --packages) packages="${2:?}"; shift 2 ;;
        --tree) trees+=("${2:?}"); shift 2 ;;
        --out) out="${2:?}"; shift 2 ;;
        --promoted) promoted="${2:?}"; shift 2 ;;
        -h|--help) usage ;;
        *) echo "unknown argument $1" >&2; usage ;;
    esac
done

if [ -n "$promoted" ]; then
    manifest="$("${CRANE:-crane}" manifest "$promoted" 2>/dev/null)" || exit 0
    printf '%s' "$manifest" | python3 -c '
import json, sys
print(json.load(sys.stdin).get("annotations", {}).get("ai.cua.image.content-digest", ""))'
    exit 0
fi

[ -n "$rootfs" ] || [ -n "$packages" ] || [ ${#trees[@]} -gt 0 ] || usage
inputs="$(mktemp)"; trap 'rm -f "$inputs"' EXIT

if [ -n "$rootfs" ]; then
    echo "# packages" >>"$inputs"
    docker run --rm --network none --entrypoint /bin/sh "$rootfs" -c '
        if command -v dpkg-query >/dev/null 2>&1; then dpkg-query -W -f "\${Package}=\${Version}\n"
        elif command -v pacman >/dev/null 2>&1; then pacman -Q | tr " " "="
        else echo "no package manager" >&2; exit 3; fi' | LC_ALL=C sort >>"$inputs" ||
        { echo "cannot list the packages of $rootfs" >&2; exit 2; }
elif [ -n "$packages" ]; then
    echo "# packages" >>"$inputs"
    LC_ALL=C sort "$packages" >>"$inputs"
fi

for t in ${trees[@]+"${trees[@]}"}; do
    h="$(git rev-parse "HEAD:$t" 2>/dev/null)" || { echo "no tree $t at HEAD" >&2; exit 2; }
    echo "tree $t $h" >>"$inputs"
done

if command -v sha256sum >/dev/null; then sum=(sha256sum); else sum=(shasum -a 256); fi
digest="sha256:$("${sum[@]}" <"$inputs" | cut -d' ' -f1)"
[ -z "$out" ] || { cp "$inputs" "$out"; echo "# digest $digest" >>"$out"; }
echo "$digest"

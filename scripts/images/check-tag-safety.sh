#!/usr/bin/env bash
# Refuse registry pushes that would overwrite tags older docs and SDKs rely on.
#
#   scripts/images/check-tag-safety.sh [--moving] [--digest sha256:...] REF...
#   scripts/images/check-tag-safety.sh --list
#
# Run it before every push. For each REF (<repo>:<tag>):
#   - tag absent                         -> ok
#   - tag present and equal to --digest  -> ok (already published)
#   - tag present, --moving, canonical repo, a floating tag (<ver>[-slim|-xcode[-X.Y]][-disk]) -> ok
#   - `latest` in a canonical repo       -> refused, present or not
#   - tag present, --moving, bench repo, a version tag (<ver>, <ver>-disk) -> ok
#   - tag present, --moving, channel repo, a channel tag (edge, rc, stable, +-disk) -> ok
#   - anything else                      -> refused, exit 1
#
# Canonical repos (ghcr.io/trycua/{linux,windows,macos}) may re-point only
# their floating tags <ver>[-<tier>][-disk] (CANONICAL_FLOAT_TAG_RE: 24.04,
# 24.04-disk, 24.04-slim-disk, 26-xcode, 26-xcode-26.1, 2022-disk, 15) and
# never get a `latest` tag; every other tag there (the
# dated pins, the per-arch children, build tags) is written once and never
# moves. ghcr.io/trycua/linux is published by cd-image-linux.yml (`cua images
# publish|promote`). Benchmark image repos
# (ghcr.io/trycua/bench-*, new repos that no older doc or SDK references) may
# re-point version tags only: <ver> and <ver>-disk, never latest/main/stable
# and never an immutable pin (scripts/bench-images/publish.sh).
# Distribution-channel repos (ghcr.io/trycua/omarchy) float only their channel
# tags (edge, edge-disk, rc, stable, ...) (`cua images promote`).
# Immutable tags (<name>-<yyyymmdd>-<sha7>[-<arch>]) never move, anywhere.
# Protected legacy repos (below) never get an existing tag re-pointed, with or
# without --moving. New unique tags there (e.g. main-<sha8>) are still allowed.
#
# Digests come from `crane digest` (CRANE=... to override), falling back to
# `docker buildx imagetools inspect`. Exit codes: 0 ok, 1 refused, 2 usage/lookup error.
set -euo pipefail

CANONICAL_REPOS=(
    ghcr.io/trycua/linux
    ghcr.io/trycua/windows
    ghcr.io/trycua/macos
)
# Shell globs on the repository (no tag).
PROTECTED_REPOS=(
    "public.ecr.aws/k5j5w0x5/*"
    "ghcr.io/trycua/macos-*-cua"
    "ghcr.io/trycua/macos-sequoia-vanilla"
    "docker.io/trycua/cua-qemu-*"
    "docker.io/trycua/xfce*"
    "docker.io/trycua/kasm*"
    "ghcr.io/trycua/cua"
    # Legacy name of ghcr.io/trycua/linux: frozen and read-only. Nothing new
    # is pushed there, and scripts/images/check-image-refs.py fails new uses.
    "ghcr.io/trycua/cua-desktop-linux"
    "*.dkr.ecr.*.amazonaws.com/desktop-workspace"
)
# Benchmark image repos: floating <ver> / <ver>-disk tags are allowed here only.
BENCH_REPOS=(
    "ghcr.io/trycua/bench-*"
)
# Distribution-channel repos: only channel tags float. Mirrored by
# libs/cua/crates/cua-image/src/publish.rs (a unit test compares the lists).
CHANNEL_REPOS=(
    "ghcr.io/trycua/omarchy"
)
CHANNEL_TAG_RE='^(edge|rc|stable)(-disk)?$'
BENCH_VERSION_TAG_RE='^[A-Za-z0-9][A-Za-z0-9._-]*$'
BENCH_RESERVED_TAG_RE='^(latest|main|stable|edge|nightly)(-disk)?$'
# Mirrored by CANONICAL_FLOAT_TAG_RE in libs/cua/crates/cua-image/src/publish.rs.
CANONICAL_FLOAT_TAG_RE='^[0-9]+(\.[0-9]+)*(-slim|-xcode(-[0-9]+(\.[0-9]+)*)?)?(-disk)?$'
IMMUTABLE_TAG_RE='-[0-9]{8}-[0-9a-f]{7}(-(amd64|arm64))?$'

usage() { sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//' >&2; exit 2; }

moving=0 want="" refs=()
while [ $# -gt 0 ]; do
    case "$1" in
        --moving) moving=1; shift ;;
        --digest) want="${2:?--digest needs a value}"; shift 2 ;;
        --list)
            printf 'canonical %s\n' "${CANONICAL_REPOS[@]}"
            printf 'protected %s\n' "${PROTECTED_REPOS[@]}"
            printf 'bench     %s\n' "${BENCH_REPOS[@]}"
            printf 'channel   %s\n' "${CHANNEL_REPOS[@]}"
            exit 0 ;;
        -h|--help) usage ;;
        -*) echo "unknown option $1" >&2; usage ;;
        *) refs+=("$1"); shift ;;
    esac
done
[ ${#refs[@]} -gt 0 ] || usage

# Normalize a repository the way docker does: bare names live on docker.io.
normalize_repo() {
    local r="$1" first="${1%%/*}"
    if [[ "$r" != */* ]]; then r="docker.io/library/$r"
    elif [[ "$first" != *.* && "$first" != *:* && "$first" != localhost ]]; then r="docker.io/$r"
    fi
    echo "$r"
}

# Prints the digest of REF, or nothing when the tag does not exist.
remote_digest() {
    local ref="$1" out
    if [ -n "${CRANE:-}" ] || command -v crane >/dev/null; then
        if out="$("${CRANE:-crane}" digest "$ref" 2>&1)"; then echo "$out"; return 0; fi
        case "$out" in
            *MANIFEST_UNKNOWN*|*NAME_UNKNOWN*|*"not found"*|*"404"*) return 0 ;;
        esac
        echo "cannot look up $ref: $out" >&2; return 2
    fi
    if out="$(docker buildx imagetools inspect "$ref" --format '{{json .Manifest}}' 2>&1)"; then
        echo "$out" | sed -n 's/.*"digest":"\(sha256:[0-9a-f]*\)".*/\1/p' | head -1; return 0
    fi
    case "$out" in *"not found"*|*MANIFEST_UNKNOWN*|*NAME_UNKNOWN*) return 0 ;; esac
    echo "cannot look up $ref: $out" >&2; return 2
}

# shellcheck disable=SC2053 # $p is a glob on purpose
in_list() { local x="$1"; shift; local p; for p in "$@"; do [[ "$x" == $p ]] && return 0; done; return 1; }

refused=0
for ref in "${refs[@]}"; do
    [[ "$ref" == *@* ]] && { echo "ok      $ref (digest ref, content-addressed)"; continue; }
    tag="${ref##*:}" repo="${ref%:*}"
    if [[ "$repo" == "$ref" || "$tag" == */* ]]; then echo "REFUSED $ref: no tag" >&2; refused=1; continue; fi
    nrepo="$(normalize_repo "$repo")"
    if [ "$tag" = latest ] && in_list "$nrepo" "${CANONICAL_REPOS[@]}"; then
        echo "REFUSED $ref: canonical repos never carry latest" >&2; refused=1; continue
    fi
    rc=0; cur="$(remote_digest "$ref")" || rc=$?
    [ "$rc" = 0 ] || exit 2
    if [ -z "$cur" ]; then echo "ok      $ref (new tag)"; continue; fi
    if [ -n "$want" ] && [ "$cur" = "$want" ]; then echo "ok      $ref (already $cur)"; continue; fi
    if in_list "$nrepo" "${PROTECTED_REPOS[@]}"; then
        echo "REFUSED $ref: protected legacy repo, tag exists at $cur" >&2; refused=1; continue
    fi
    if [[ "$tag" =~ $IMMUTABLE_TAG_RE ]]; then
        echo "REFUSED $ref: immutable tag exists at $cur" >&2; refused=1; continue
    fi
    if [ "$moving" = 1 ] && in_list "$nrepo" "${CANONICAL_REPOS[@]}"; then
        if [[ "$tag" =~ $CANONICAL_FLOAT_TAG_RE ]]; then
            echo "ok      $ref (floating tag, was $cur)"; continue
        fi
        echo "REFUSED $ref: canonical repos float only <ver>[-slim|-xcode[-X.Y]][-disk] tags" >&2; refused=1; continue
    fi
    if [ "$moving" = 1 ] && in_list "$nrepo" "${BENCH_REPOS[@]}"; then
        if [[ "$tag" =~ $BENCH_VERSION_TAG_RE ]] && ! [[ "$tag" =~ $BENCH_RESERVED_TAG_RE ]]; then
            echo "ok      $ref (bench version tag, was $cur)"; continue
        fi
        echo "REFUSED $ref: bench repos float only <ver> and <ver>-disk tags" >&2; refused=1; continue
    fi
    if [ "$moving" = 1 ] && in_list "$nrepo" "${CHANNEL_REPOS[@]}"; then
        if [[ "$tag" =~ $CHANNEL_TAG_RE ]]; then
            echo "ok      $ref (channel tag, was $cur)"; continue
        fi
        echo "REFUSED $ref: channel repos float only their channel tags" >&2; refused=1; continue
    fi
    echo "REFUSED $ref: tag exists at $cur (pass --moving only for canonical, bench version or channel tags)" >&2
    refused=1
done
exit "$refused"

#!/usr/bin/env bash
# Pin a Cua Spaces nightly to the commit it was built from.
#
#   spaces_nightly_tag.sh <tag> <sha>          create the tag (lightweight, via the git refs API)
#   spaces_nightly_tag.sh --dry-run <tag> <sha>  only print what would happen (no write)
#
# Needs GH_TOKEN and GITHUB_REPOSITORY. The tag is immutable by design: it is
# created once and never moved or force-updated. If it already exists at the same
# commit (a re-run of the same job) that is fine; if it points anywhere else the
# script fails loudly and changes nothing.
set -euo pipefail

dry=false
if [ "${1:-}" = "--dry-run" ]; then dry=true; shift; fi
tag="${1:?usage: spaces_nightly_tag.sh [--dry-run] <tag> <sha>}"
sha="${2:?usage: spaces_nightly_tag.sh [--dry-run] <tag> <sha>}"
repo="${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is not set}"

[[ "$tag" =~ ^cua-spaces-nightly-[0-9]{8}\.[0-9]+$ ]] || { echo "::error::bad nightly tag name: $tag"; exit 1; }
[[ "$sha" =~ ^[0-9a-f]{40}$ ]] || { echo "::error::bad commit sha: $sha"; exit 1; }

# Prints "<type> <sha>" of the tag ref, or nothing when the tag does not exist.
lookup() {
  local out
  if out="$(gh api "repos/$repo/git/ref/tags/$tag" --jq '.object.type + " " + .object.sha' 2>&1)"; then
    echo "$out"
  elif grep -qiE 'not found|404' <<<"$out"; then
    return 0
  else
    echo "::error::could not look up tag $tag: $out" >&2
    return 1
  fi
}

check_same() {
  local found="$1"
  if [ "$found" = "commit $sha" ]; then
    echo "Tag $tag already points at $sha: nothing to do."
    return 0
  fi
  echo "::error::tag $tag already exists and points at ${found#* } (${found%% *}), not at $sha. Nightly tags are immutable: not touching it."
  return 1
}

existing="$(lookup)"
if [ -n "$existing" ]; then check_same "$existing"; exit $?; fi

if [ "$dry" = true ]; then
  echo "DRY RUN: would create lightweight tag refs/tags/$tag at $sha in $repo."
  exit 0
fi

if gh api -X POST "repos/$repo/git/refs" -f "ref=refs/tags/$tag" -f "sha=$sha" >/dev/null; then
  echo "Created tag $tag at $sha."
else
  # Lost a race with another creator: accept only the identical result.
  existing="$(lookup)"
  [ -n "$existing" ] || { echo "::error::could not create tag $tag"; exit 1; }
  check_same "$existing"
fi

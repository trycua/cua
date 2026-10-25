#!/usr/bin/env bash
# Prints the real notes for a Cua Spaces version on stdout.
#
#   spaces_release_notes.sh <version>      (GH_TOKEN and GITHUB_REPOSITORY set)
#
# Release Please writes the notes into a draft titled "cua-spaces: v<version>",
# which GitHub lists as "untagged-..." and which `gh release view <tag>` does not
# find. Use that body first, then this version's CHANGELOG.md section, then the
# generic line.
set -euo pipefail
version="$1"
repo="${GITHUB_REPOSITORY:-trycua/cua}"
body="$(gh api --paginate "repos/$repo/releases?per_page=100" \
  --jq ".[] | select(.draft and .name == \"cua-spaces: v$version\") | .body" 2>/dev/null | head -c 100000 || true)"
if [ -n "${body//[[:space:]]/}" ]; then
  printf '%s\n' "$body"
  exit 0
fi
here="$(cd "$(dirname "$0")/../.." && pwd)"
changelog="$here/apps/cua-spaces-macos/CHANGELOG.md"
if [ -f "$changelog" ]; then
  python3 "$here/apps/cua-spaces-macos/scripts/changelog-notes.py" "$version" "$changelog"
else
  echo "Cua Spaces $version."
fi

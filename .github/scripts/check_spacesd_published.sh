#!/usr/bin/env bash
# Fails unless the cua-spacesd release the `cua` CLI bakes in
# (libs/cua-spacesd/VERSION, read by cua-host's driver.rs) is PUBLISHED on
# REPO with every artifact `cua host setup` downloads, each reachable
# anonymously, the way an end user's machine fetches it.
#
# Cua Spaces 0.3.0 shipped a CLI pinned to cua-spacesd 0.2.0, a release that
# stayed an empty draft, so "Set up for access" 404'd for everyone. Release
# workflows that bake the CLI (cd-cua-spaces.yml, cd-cua-sdk.yml) and the
# Release Please PR check run this before building anything.
#
# Usage: check_spacesd_published.sh [REPO]   (default: $GITHUB_REPOSITORY,
#        else trycua/cua). Needs `gh` (GH_TOKEN) and curl.
# A prerelease counts as published only off trycua/cua (staging publishes
# its cua-spacesd releases as prereleases).
set -euo pipefail

REPO="${1:-${GITHUB_REPOSITORY:-trycua/cua}}"
ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
VERSION="$(tr -d '[:space:]' < "$ROOT/libs/cua-spacesd/VERSION")"
TAG="cua-spacesd-v$VERSION"

# cua-host's artifact_name(): cua-spacesd-<os>-<arch>.tar.gz plus the
# .sha256 it requires, for each platform cd-cua-spacesd.yml publishes.
ASSETS=()
for p in macos-aarch64 macos-x86_64 linux-x86_64 linux-aarch64 windows-x86_64; do
  ASSETS+=("cua-spacesd-$p.tar.gz" "cua-spacesd-$p.tar.gz.sha256")
done

fail() {
  echo "::error title=cua-spacesd $VERSION is not published::$*"
  echo "This build bakes cua-spacesd $VERSION (libs/cua-spacesd/VERSION) into the cua CLI," >&2
  echo "and \`cua host setup\` downloads it from https://github.com/$REPO/releases/tag/$TAG." >&2
  echo "Publish that release first (CD: cua-spacesd, or merge its Release Please PR)," >&2
  echo "or set libs/cua-spacesd/VERSION back to a published release." >&2
  exit 1
}

# The API answers 404 for a draft looked up by tag.
if ! json="$(gh api "repos/$REPO/releases/tags/$TAG" 2>/dev/null)"; then
  fail "$REPO has no published release $TAG (missing, or still a draft)"
fi
[ "$(jq -r .draft <<<"$json")" = false ] || fail "$TAG is still a draft"
if [ "$REPO" = trycua/cua ] && [ "$(jq -r .prerelease <<<"$json")" != false ]; then
  fail "$TAG is a prerelease; a release build must pin a full cua-spacesd release"
fi

missing=()
for name in "${ASSETS[@]}"; do
  url="$(jq -r --arg n "$name" '.assets[] | select(.name == $n) | .browser_download_url' <<<"$json")"
  if [ -z "$url" ]; then
    missing+=("$name (not attached)")
  elif ! curl -fsSIL --retry 3 --max-time 60 -o /dev/null "$url"; then
    missing+=("$name (not downloadable: $url)")
  fi
done
if [ ${#missing[@]} -gt 0 ]; then
  fail "$TAG lacks: ${missing[*]}"
fi
echo "cua-spacesd $VERSION is published on $REPO with all ${#ASSETS[@]} host-setup assets."

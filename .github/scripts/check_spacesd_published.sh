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
# The static headless builds the CLI and the app bundle to inject into OCI
# images (fetch_static_spacesd.sh).
for a in x86_64 aarch64; do
  ASSETS+=("cua-spacesd-$a-unknown-linux-musl" "cua-spacesd-$a-unknown-linux-musl.sha256")
done

fail() {
  echo "::error title=cua-spacesd $VERSION is not published::$*"
  echo "This build bakes cua-spacesd $VERSION (libs/cua-spacesd/VERSION) into the cua CLI," >&2
  echo "and \`cua host setup\` downloads it from https://github.com/$REPO/releases/tag/$TAG." >&2
  echo "Publish that release first (CD: cua-spacesd, or merge its Release Please PR)," >&2
  echo "or set libs/cua-spacesd/VERSION back to a published release." >&2
  exit 1
}

# The API answers 404 for a draft looked up by tag. A push to main that bumps
# libs/cua-spacesd/VERSION starts this check at the same moment as
# cd-cua-spacesd.yml starts publishing that release (5 Nov 2026: the main
# build failed on a release that appeared minutes later), so a caller that
# builds on main can set SPACESD_WAIT_MINUTES to wait for it.
WAIT_MINUTES="${SPACESD_WAIT_MINUTES:-0}"
deadline=$(( $(date +%s) + WAIT_MINUTES * 60 ))
until json="$(gh api "repos/$REPO/releases/tags/$TAG" 2>/dev/null)"; do
  if [ "$(date +%s)" -ge "$deadline" ]; then
    fail "$REPO has no published release $TAG (missing, or still a draft)"
  fi
  echo "$TAG is not published yet; checking again in 60 s"
  sleep 60
done
[ "$(jq -r .draft <<<"$json")" = false ] || fail "$TAG is still a draft"
if [ "$REPO" = trycua/cua ] && [ "$(jq -r .prerelease <<<"$json")" != false ]; then
  fail "$TAG is a prerelease; a release build must pin a full cua-spacesd release"
fi

missing=()
for name in "${ASSETS[@]}"; do
  url="$(jq -r --arg n "$name" '.assets[] | select(.name == $n) | .browser_download_url' <<<"$json")"
  if [ -z "$url" ]; then
    missing+=("$name (not attached)")
  # GitHub's release CDN answers 504 for minutes at a time (5 Nov 2026):
  # retry transient errors with a pause before calling an asset missing.
  elif ! curl -fsSIL --retry 6 --retry-delay 15 --retry-max-time 300 --max-time 60 -o /dev/null "$url"; then
    missing+=("$name (not downloadable: $url)")
  fi
done
if [ ${#missing[@]} -gt 0 ]; then
  fail "$TAG lacks: ${missing[*]}"
fi
echo "cua-spacesd $VERSION is published on $REPO with all ${#ASSETS[@]} host-setup assets."

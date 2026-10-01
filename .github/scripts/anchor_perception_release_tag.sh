#!/usr/bin/env bash
# Anchor the merged Cua Perception version with a lightweight tag on main.
#
# Runs on every push to main from release-please.yml with the release app
# token, so the tag push starts `cd-cua-perception.yml`. When a push changes
# VERSION, the pushed commit is tagged. When VERSION is unchanged but its tag
# is missing (a previous anchor run failed), the commit that last set VERSION
# on main's first-parent history is tagged instead, so a missed tag heals on
# the next push without anyone creating it by hand.
#
# Inputs (environment): GITHUB_REPOSITORY, GITHUB_SHA, BEFORE_SHA, VERSION_PATH,
# GH_TOKEN (used by gh). Optional: RELEASE_VERSION_VALIDATOR, the command that
# validates checked-in versions (defaults to validate_release_versions.py).
set -euo pipefail

: "${GITHUB_REPOSITORY:?}" "${GITHUB_SHA:?}" "${BEFORE_SHA:?}" "${VERSION_PATH:?}"
VALIDATOR="${RELEASE_VERSION_VALIDATOR:-python3 .github/scripts/validate_release_versions.py --product perception}"

git checkout --quiet --detach "$GITHUB_SHA"

if [[ "$BEFORE_SHA" =~ ^0+$ ]]; then
  echo "Skipping Perception tag creation for the initial branch push"
  exit 0
fi
if ! git merge-base --is-ancestor "$BEFORE_SHA" "$GITHUB_SHA"; then
  echo "::error::Refusing to create a release tag for a non-fast-forward main push"
  exit 1
fi

read_version() {
  git show "$1:$VERSION_PATH" 2>/dev/null | tr -d '[:space:]' || true
}

require_increase() {
  local previous="$1" current="$2"
  python3 - "$previous" "$current" <<'PY'
import re
import sys

semver = re.compile(r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")
previous, current = sys.argv[1:]
previous_match = semver.fullmatch(previous)
current_match = semver.fullmatch(current)
if not previous_match or not current_match:
    raise SystemExit("Perception versions must be stable SemVer")
if tuple(map(int, current_match.groups())) <= tuple(map(int, previous_match.groups())):
    raise SystemExit(
        f"Perception version must increase: previous={previous}, current={current}"
    )
PY
}

BEFORE_VERSION=$(read_version "$BEFORE_SHA")
VERSION=$(read_version "$GITHUB_SHA")
if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "::error::Refusing to tag invalid Perception version: $VERSION"
  exit 1
fi
TAG="cua-perception-v$VERSION"

# Print "<type>\t<sha>" for an existing tag, nothing when it does not exist,
# and fail on any other error. gh prints the error body on stdout for a 404,
# so the exit status and body are checked explicitly instead of trusting
# non-empty output.
read_remote_tag() {
  local body status=0
  body=$(gh api "repos/$GITHUB_REPOSITORY/git/ref/tags/$TAG" 2>/dev/null) || status=$?
  if [[ "$status" -eq 0 ]]; then
    jq -r '[.object.type, .object.sha] | @tsv' <<<"$body"
    return 0
  fi
  if jq -e '.status == "404" or .message == "Not Found"' <<<"$body" >/dev/null 2>&1; then
    return 0
  fi
  echo "::error::Could not read $TAG (gh exit $status)" >&2
  return 1
}

require_exact_lightweight_tag() {
  local tag_record="$1" expected_sha="$2"
  local tag_type tag_sha
  IFS=$'\t' read -r tag_type tag_sha <<<"$tag_record"
  if [[ "$tag_type" != "commit" || "$tag_sha" != "$expected_sha" ]]; then
    echo "::error::$TAG already exists but is not a lightweight tag at $expected_sha"
    exit 1
  fi
}

EXISTING=$(read_remote_tag)
if [[ "$BEFORE_VERSION" == "$VERSION" ]]; then
  if [[ -n "$EXISTING" ]]; then
    echo "Perception version did not change in this main push and $TAG exists"
    exit 0
  fi
  TARGET_SHA=$(git log --first-parent -1 --format=%H "$GITHUB_SHA" -- "$VERSION_PATH")
  if [[ -z "$TARGET_SHA" ]]; then
    echo "::error::No main commit sets $VERSION_PATH to $VERSION"
    exit 1
  fi
  if [[ "$(read_version "$TARGET_SHA")" != "$VERSION" ]]; then
    echo "::error::$TARGET_SHA does not set Perception $VERSION"
    exit 1
  fi
  PARENT_VERSION=$(read_version "$TARGET_SHA^")
  if [[ -n "$PARENT_VERSION" ]]; then
    require_increase "$PARENT_VERSION" "$VERSION"
  fi
  echo "::warning::$TAG is missing; anchoring the commit that set VERSION ($TARGET_SHA)"
else
  $VALIDATOR
  if [[ -n "$BEFORE_VERSION" ]]; then
    require_increase "$BEFORE_VERSION" "$VERSION"
  fi
  TARGET_SHA="$GITHUB_SHA"
fi

git fetch --quiet origin "+refs/heads/main:refs/remotes/origin/main"
if ! git merge-base --is-ancestor "$TARGET_SHA" origin/main; then
  echo "::error::Refusing to tag a commit outside main"
  exit 1
fi

if [[ -n "$EXISTING" ]]; then
  require_exact_lightweight_tag "$EXISTING" "$TARGET_SHA"
  echo "$TAG already points to the merged Perception version"
  exit 0
fi

if ! gh api --method POST "repos/$GITHUB_REPOSITORY/git/refs" \
  -f ref="refs/tags/$TAG" \
  -f sha="$TARGET_SHA" >/dev/null; then
  # A concurrent run may have created the same immutable anchor.
  EXISTING=$(read_remote_tag)
  if [[ -z "$EXISTING" ]]; then
    echo "::error::Failed to create $TAG"
    exit 1
  fi
  require_exact_lightweight_tag "$EXISTING" "$TARGET_SHA"
fi
require_exact_lightweight_tag "$(read_remote_tag)" "$TARGET_SHA"
echo "Created $TAG at $TARGET_SHA"

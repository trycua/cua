#!/usr/bin/env bash
# Make release tags resolvable in a cua-staging checkout without pushing them.
#
#   ensure_upstream_tag.sh TAG...
#
# Staging shares trycua/cua's history but not its release tags, and pushing
# them here would start the stable release workflows. For each TAG missing
# from the local clone, fetch it read-only from the public repository into
# this runner's refs/tags only. Tags that already resolve are left alone.
set -euo pipefail
UPSTREAM="${CUA_RELEASE_TAG_UPSTREAM:-https://github.com/trycua/cua.git}"
for tag in "$@"; do
    [ -n "$tag" ] || continue
    if git rev-parse -q --verify "refs/tags/$tag^{commit}" >/dev/null; then
        continue
    fi
    echo "fetching $tag from $UPSTREAM (local ref only)"
    git fetch --no-tags --quiet "$UPSTREAM" "refs/tags/$tag:refs/tags/$tag"
done

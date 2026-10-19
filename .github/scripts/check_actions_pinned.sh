#!/usr/bin/env bash
# Fail when an external GitHub Action is referenced by tag or branch instead of a
# full 40-character commit SHA. Local actions (./...) and docker:// refs are exempt.
#
# Used by CI (ci-lint-workflows.yml) and the lefthook pre-push hook. To fix a hit:
#   uses: owner/repo@v4  ->  uses: owner/repo@<40-char sha> # v4
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

dirs=()
for d in .github/workflows .github/actions; do
  [ -d "$d" ] && dirs+=("$d")
done
[ "${#dirs[@]}" -eq 0 ] && exit 0

unpinned=$(
  grep -rnE '^[[:space:]]*(-[[:space:]]+)?uses:[[:space:]]*[^[:space:]#]+' "${dirs[@]}" \
    | grep -vE 'uses:[[:space:]]*(\./|docker://)' \
    | grep -vE '@[0-9a-f]{40}([[:space:]]|$)' \
    || true
)

if [ -n "$unpinned" ]; then
  echo "Actions must be pinned to a full commit SHA (add the tag as a trailing comment):" >&2
  echo "$unpinned" >&2
  exit 1
fi

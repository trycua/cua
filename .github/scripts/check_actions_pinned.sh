#!/usr/bin/env bash
# Fail when an external GitHub Action is referenced by tag or branch instead of a
# full 40-character commit SHA. Local actions (./...) and docker:// refs are exempt.
#
# Used by CI (the workflow lint job) and the lefthook pre-push hook. To fix a hit:
#   uses: owner/repo@v4  ->  uses: owner/repo@<40-char sha> # v4
set -u

cd "$(git rev-parse --show-toplevel)" || exit 2

dirs=()
for d in .github/workflows .github/actions; do
  [ -d "$d" ] && dirs+=("$d")
done
[ "${#dirs[@]}" -eq 0 ] && exit 0

unpinned=$(
  find "${dirs[@]}" -type f \( -name '*.yml' -o -name '*.yaml' \) -print0 |
    xargs -0 awk '
      /^[[:space:]]*(-[[:space:]]+)?uses:[[:space:]]*/ {
        ref = $0
        sub(/^[[:space:]]*(-[[:space:]]+)?uses:[[:space:]]*/, "", ref)
        sub(/[[:space:]#].*$/, "", ref)
        if (substr(ref, 1, 2) == "./" || substr(ref, 1, 9) == "docker://") next
        n = split(ref, parts, "@")
        sha = parts[n]
        if (n > 1 && length(sha) == 40 && sha ~ /^[0-9a-f]+$/) next
        print FILENAME ":" FNR ": " $0
      }'
)

if [ -n "$unpinned" ]; then
  echo "Actions must be pinned to a full commit SHA (add the tag as a trailing comment):" >&2
  echo "$unpinned" >&2
  exit 1
fi

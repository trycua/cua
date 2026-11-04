#!/usr/bin/env bash
# Docs link to trycua/cua main, but this repo (cua-staging) ships first: a
# path added here 404s on trycua/cua until it is released. A trycua/cua
# link whose path exists in this checkout is one of those; this prints an
# exact-match lychee ignore line for each, so the external link check skips
# them and still checks every other link (a typo'd path is not in the tree
# and stays checked).
set -euo pipefail
root="$(git rev-parse --show-toplevel)"
cd "$root"
grep -rhoE 'https://github\.com/trycua/cua/(blob|tree)/main/[^ )"'"'"'<>`#?]+' docs/content \
  | sort -u \
  | while read -r url; do
      path="${url#https://github.com/trycua/cua/}"
      path="${path#blob/main/}"
      path="${path#tree/main/}"
      path="${path%/}"
      if [ -e "$path" ]; then
        # lychee ignore lines are regexes: escape them and anchor both ends.
        printf '^%s/?(#.*)?$\n' "$(printf '%s' "$url" | sed 's/[.[\*^$()+?{|\\]/\\&/g')"
      fi
    done

#!/usr/bin/env bash
# Docs style gate: no em dashes (U+2014) in the docs site or repo Markdown.
# Use commas, colons, parentheses or separate sentences instead.
#
#   scripts/check-no-em-dash.sh            # every tracked *.md / *.mdx
#   scripts/check-no-em-dash.sh FILE...    # only these files
#
# Excluded: historical or mirrored text we do not rewrite (CHANGELOGs, release
# notes and backfill, blog posts, dated changelog entries, the read-only
# libs/fleet mirror). Generated reference pages are covered: their generators
# pass output through scripts/docs-generators/prose-style.ts.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

EXCLUDE='(^|/)CHANGELOG[^/]*\.md$|^\.github/release-backfill/|^\.github/release-notes/|^blog/|^changelog/|^libs/fleet/'
EM=$'\xe2\x80\x94'

if [ "$#" -gt 0 ]; then
  files=$(printf '%s\n' "$@")
else
  files=$(git ls-files -- '*.md' '*.mdx')
fi
files=$(printf '%s\n' "$files" | grep -Ev "$EXCLUDE" | grep -E '\.(md|mdx)$' || true)

[ -z "$files" ] && { echo "No Markdown files to check."; exit 0; }

hits=$(printf '%s\n' "$files" | tr '\n' '\0' | xargs -0 grep -Hn -- "$EM" 2>/dev/null || true)
if [ -n "$hits" ]; then
  echo "Em dashes found. Rewrite with commas, colons, parentheses or separate sentences:"
  echo
  echo "$hits"
  echo
  echo "$(printf '%s\n' "$hits" | wc -l | tr -d ' ') line(s) in $(printf '%s\n' "$hits" | cut -d: -f1 | sort -u | wc -l | tr -d ' ') file(s)."
  exit 1
fi
echo "No em dashes in $(printf '%s\n' "$files" | wc -l | tr -d ' ') Markdown files."

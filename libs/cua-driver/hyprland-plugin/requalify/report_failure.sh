#!/usr/bin/env bash
# Open or update ONE tracking issue for a failed Hyprland plugin requalification.
#
#   report_failure.sh <run url> <failures.md> [matrix.md]
#
# The issue is keyed by its title and the `hyprland-requalify` label: an open
# one gets a comment, otherwise a new one is opened. A later passing run does
# not close it; whoever fixes the cause does. Needs gh with GH_TOKEN
# (issues: write) and GH_REPO or a checkout. DRY_RUN=1 prints the gh calls.
set -euo pipefail
url="${1:?run url}" failures="${2:?failures file}" matrix="${3:-}"
title="Hyprland plugin requalification failing"
label=hyprland-requalify
gh_() { if [ "${DRY_RUN:-0}" = 1 ]; then printf 'gh'; printf ' %q' "$@"; printf '\n'; else gh "$@"; fi; }

body="Requalification run: $url

$(cat "$failures")"
if [ -n "$matrix" ] && [ -f "$matrix" ]; then
    body="$body

<details><summary>Matrix</summary>

$(cat "$matrix")
</details>"
fi

existing=""
if [ "${DRY_RUN:-0}" != 1 ]; then
    existing="$(gh issue list --state open --label "$label" --search "\"$title\" in:title" \
        --json number,title --jq ".[] | select(.title == \"$title\") | .number" 2>/dev/null | head -1 || true)"
fi
if [ -n "$existing" ]; then
    gh_ issue comment "$existing" --body "$body"
    echo "commented on #$existing"
else
    gh_ label create "$label" --color B60205 --description "Hyprland plugin requalification failures" 2>/dev/null || true
    gh_ issue create --title "$title" --label "$label" --body "$body

This issue collects failed requalification runs (one issue, a comment per failing run). Refs #4909."
    echo "opened an issue"
fi

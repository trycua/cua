#!/usr/bin/env bash
# Open or update ONE tracking issue for a failed scheduled image rebuild.
#
#   scripts/images/report-schedule-failure.sh <workflow name> <run url> [details]
#
# The issue is keyed by its title ("Scheduled image rebuild failing: <workflow
# name>") and the `image-schedule` label: an open one gets a comment, else a new
# one is opened. A later successful run does not close it; whoever fixes the
# cause does. Needs gh with GH_TOKEN (issues: write) and GH_REPO or a checkout.
# DRY_RUN=1 prints the gh calls instead.
set -euo pipefail
name="${1:?workflow name}" url="${2:?run url}" details="${3:-}"
title="Scheduled image rebuild failing: $name"
label=image-schedule
gh_() { if [ "${DRY_RUN:-0}" = 1 ]; then echo "gh $*"; else gh "$@"; fi; }

body="The scheduled run failed: $url"
[ -z "$details" ] || body="$body

$details"

existing="$(gh issue list --state open --label "$label" --search "\"$title\" in:title" \
    --json number,title --jq ".[] | select(.title == \"$title\") | .number" 2>/dev/null | head -1 || true)"
if [ -n "$existing" ]; then
    gh_ issue comment "$existing" --body "$body"
    echo "commented on #$existing"
else
    gh_ label create "$label" --color B60205 --description "Scheduled image rebuild failures" 2>/dev/null || true
    gh_ issue create --title "$title" --label "$label" --body "$body

This issue collects every failed scheduled run of this workflow (one issue, comments per run)."
    echo "opened an issue"
fi

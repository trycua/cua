#!/usr/bin/env bash
# Staging-first gate (CUA-1907). Exits non-zero unless a green staging smoke is
# recorded for this exact candidate, or the break-glass input is set.
#
#   require_staging_smoke.sh <component> <sha>
#
# Evidence: a commit status on <sha> with context "staging-smoke/<component>"
# and state "success", posted by the staging smoke runner. Its description
# carries the candidate's artifact digest ("sha256:<hex> ..."), which is shown
# in the summary. A status is bound to its commit, so a smoke of another SHA
# never satisfies the gate. Missing, pending, failed or older than MAX_AGE_HOURS
# (default 336) evidence fails.
#
# How to satisfy it (the smoke lives in trycua/cloud and is run by hand; no
# workflow runs it for you). After the release PR merges and the tag exists:
#   GH_TOKEN=<statuses:write on trycua/cua> python3 scripts/spaces-staging/smoke.py \
#     --post-status <component> --sha <tag commit> --digest sha256:<artifact set>
# then re-run the failed "staging smoke is green" job on the tag run; the
# release jobs continue. For Cua Driver, the same command with
# --post-status cua-driver (its smoke is the same staging smoke).
# Driver has no workflow_dispatch bypass (its release job is tag-push only):
# its break-glass is an admin posting the status by hand with the reason in the
# description (the digest check still needs a "sha256:..." token).
#
# Break-glass (Spaces): SKIP_STAGING_GATE=true with a non-empty SKIP_REASON passes, with
# an error-level annotation and a summary line; without a reason it fails.
set -euo pipefail

component="${1:?component}"
sha="${2:?sha}"
repo="${GITHUB_REPOSITORY:?}"
summary="${GITHUB_STEP_SUMMARY:-/dev/null}"

if [ "${SKIP_STAGING_GATE:-false}" = true ]; then
  if [ -z "${SKIP_REASON// }" ]; then
    echo "::error title=Staging gate::skip_staging_gate needs a reason (skip_staging_reason)"
    exit 1
  fi
  msg="STAGING GATE BYPASSED for $component at $sha by ${GITHUB_ACTOR:-unknown}: $SKIP_REASON"
  echo "::error title=BREAK-GLASS::$msg"
  echo "### :rotating_light: $msg" >> "$summary"
  exit 0
fi

context="staging-smoke/$component"
max_age=$(( ${MAX_AGE_HOURS:-336} * 3600 ))
# Newest status per context first (combined status API).
row="$(gh api "repos/$repo/commits/$sha/status" --paginate \
  --jq ".statuses[] | select(.context == \"$context\") | [.state, .updated_at, .description, .target_url] | @tsv" \
  | head -n1 || true)"

if [ -z "$row" ]; then
  echo "::error title=Staging gate::no '$context' status on $sha. Run the staging smoke for this commit first."
  exit 1
fi
IFS=$'\t' read -r state updated desc url <<<"$row"
if [ "$state" != success ]; then
  echo "::error title=Staging gate::'$context' on $sha is '$state', not success ($url)"
  exit 1
fi
age=$(( $(date -u +%s) - $(date -u -d "$updated" +%s) ))
if [ "$age" -gt "$max_age" ]; then
  echo "::error title=Staging gate::'$context' on $sha is stale ($updated)"
  exit 1
fi
case "$desc" in
  *sha256:*) ;;
  *) echo "::error title=Staging gate::'$context' on $sha has no artifact digest in its description"; exit 1 ;;
esac
echo "Staging smoke green for $component at $sha: $desc ($url)"
echo "Staging gate: $component $sha green ($desc) $url" >> "$summary"

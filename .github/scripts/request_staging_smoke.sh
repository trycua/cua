#!/usr/bin/env bash
# Starts the staging smoke for a release candidate (CUA-1993). Sends a
# repository_dispatch of type cua-release-candidate to trycua/cloud, whose
# spaces-staging-smoke workflow runs the smoke and posts the commit status
# "staging-smoke/<component>" that require_staging_smoke.sh waits for.
#
#   GH_TOKEN=<contents:write on trycua/cloud> request_staging_smoke.sh <component> <sha>
#
# Fails (its job, not the release: publishing waits on the staging gate,
# not on this job) when the dispatch cannot be sent, so a release app that
# cannot reach trycua/cloud shows as a red job instead of a warning nobody
# reads. The gate then waits for the smoke trycua/cloud's own tag poller
# starts, or one started by hand
# (gh workflow run spaces-staging-smoke.yml -R trycua/cloud ...).
set -uo pipefail

component="${1:?component}"
sha="${2:?sha}"
summary="${GITHUB_STEP_SUMMARY:-/dev/null}"

if [ -z "${GH_TOKEN:-}" ]; then
  echo "::error title=Staging smoke::no token for trycua/cloud (the release app could not mint one: is it installed on trycua/cloud with contents:write?); start the smoke by hand: gh workflow run spaces-staging-smoke.yml -R trycua/cloud -f component=$component -f sha=$sha"
  exit 1
fi
if gh api repos/trycua/cloud/dispatches \
    -f event_type=cua-release-candidate \
    -f "client_payload[component]=$component" \
    -f "client_payload[sha]=$sha" >/dev/null; then
  echo "Requested the staging smoke for $component at $sha" | tee -a "$summary"
else
  echo "::error title=Staging smoke::could not send the cua-release-candidate dispatch to trycua/cloud (the release app needs contents:write there); start the smoke by hand: gh workflow run spaces-staging-smoke.yml -R trycua/cloud -f component=$component -f sha=$sha"
  exit 1
fi

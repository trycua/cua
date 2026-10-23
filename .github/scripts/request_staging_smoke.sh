#!/usr/bin/env bash
# Starts the staging smoke for a release candidate (CUA-1993). Sends a
# repository_dispatch of type cua-release-candidate to trycua/cloud, whose
# spaces-staging-smoke workflow runs the smoke and posts the commit status
# "staging-smoke/<component>" that require_staging_smoke.sh waits for.
#
#   GH_TOKEN=<contents:write on trycua/cloud> request_staging_smoke.sh <component> <sha>
#
# Never fails the release run: if the dispatch cannot be sent it warns, and the
# gate then waits until someone starts the smoke by hand
# (gh workflow run spaces-staging-smoke.yml -R trycua/cloud ...).
set -uo pipefail

component="${1:?component}"
sha="${2:?sha}"
summary="${GITHUB_STEP_SUMMARY:-/dev/null}"

if [ -z "${GH_TOKEN:-}" ]; then
  echo "::warning title=Staging smoke::no token for trycua/cloud; start the smoke by hand: gh workflow run spaces-staging-smoke.yml -R trycua/cloud -f component=$component -f sha=$sha"
  exit 0
fi
if gh api repos/trycua/cloud/dispatches \
    -f event_type=cua-release-candidate \
    -f "client_payload[component]=$component" \
    -f "client_payload[sha]=$sha" >/dev/null; then
  echo "Requested the staging smoke for $component at $sha" | tee -a "$summary"
else
  echo "::warning title=Staging smoke::could not send the dispatch to trycua/cloud; start the smoke by hand: gh workflow run spaces-staging-smoke.yml -R trycua/cloud -f component=$component -f sha=$sha"
fi
exit 0

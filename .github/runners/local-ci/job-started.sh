#!/bin/bash
# Runner job-started hook for the `cua-local-ci` self-hosted Mac runners (ACTIONS_RUNNER_HOOK_JOB_STARTED).
#
# trycua/cua is public. A pull request's workflow files come from the pull request itself, so a fork
# can edit any workflow to target this runner's label. The `if:` in ci-local-signoff.yml is therefore
# not a security boundary on its own; this hook is. It is installed root-owned outside the runner
# directory (setup.sh), so no job can change it, and it runs before the first step of every job.
# A non-zero exit fails the job before any of its code runs.
#
# It accepts exactly one kind of job: ci-local-signoff.yml, for a pull_request event of trycua/cua
# whose head branch lives in trycua/cua itself. Everything else is refused: fork pull requests,
# other workflows, other events, other repositories.
set -euo pipefail

REPO=trycua/cua
WORKFLOW_PATH=.github/workflows/ci-local-signoff.yml

refuse() { echo "cua-local-ci guard: refusing this job: $*" >&2; exit 1; }

[ "${GITHUB_REPOSITORY:-}" = "$REPO" ] || refuse "repository '${GITHUB_REPOSITORY:-}' is not $REPO"
[ "${GITHUB_EVENT_NAME:-}" = pull_request ] || refuse "event '${GITHUB_EVENT_NAME:-}' is not pull_request"
case "${GITHUB_WORKFLOW_REF:-}" in
  "$REPO/$WORKFLOW_PATH@"*) ;;
  *) refuse "workflow '${GITHUB_WORKFLOW_REF:-}' is not $WORKFLOW_PATH" ;;
esac
[ -f "${GITHUB_EVENT_PATH:-}" ] || refuse "no event payload"

# The event payload is written by the runner from the job message GitHub sends; a job cannot edit it
# before this hook runs. Read the repositories from it, not from anything the workflow controls.
read -r base head < <(/usr/bin/python3 -I -c '
import json, sys
e = json.load(open(sys.argv[1]))
pr = e.get("pull_request") or {}
base = ((pr.get("base") or {}).get("repo") or {}).get("full_name") or ""
head = ((pr.get("head") or {}).get("repo") or {}).get("full_name") or ""
print(base or "-", head or "-")
' "$GITHUB_EVENT_PATH") || refuse "cannot read the event payload"

[ "$base" = "$REPO" ] || refuse "pull request base '$base' is not $REPO"
[ "$head" = "$REPO" ] || refuse "pull request head '$head' is not $REPO (fork pull requests never run on our Macs)"

echo "cua-local-ci guard: accepted $GITHUB_WORKFLOW_REF for a $REPO branch"

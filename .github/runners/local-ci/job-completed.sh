#!/bin/bash
# Runner job-completed hook for the `cua-local-ci` runners (ACTIONS_RUNNER_HOOK_JOB_COMPLETED).
# Empties the runner's work directory after every job, pass or fail, so the next job starts from a
# clean checkout and nothing a job wrote is left for the next one. run-instance.sh does the same at
# service start, which covers a job that died with the machine.
set -uo pipefail
dir=${CUA_RUNNER_DIR:?CUA_RUNNER_DIR is set by the launchd job}
case "$dir" in /Users/ghrunner/runner-[0-9]) ;; *) echo "cua-local-ci cleanup: unexpected runner dir '$dir'" >&2; exit 0 ;; esac
[ -d "$dir/_work" ] && find "$dir/_work" -mindepth 1 -maxdepth 1 -exec rm -rf {} +
echo "cua-local-ci cleanup: emptied the work directory"
exit 0

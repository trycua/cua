#!/bin/bash
# launchd entry point for one `cua-local-ci` runner instance (runs as the unprivileged ghrunner user).
# Empties the work directory left by a job that died with the machine, then runs the runner in the
# foreground; launchd restarts it if it exits (KeepAlive).
set -uo pipefail
n=${1:?usage: run-instance.sh <instance number>}
case "$n" in [0-9]) ;; *) echo "bad instance '$n'" >&2; exit 2 ;; esac
dir=/Users/ghrunner/runner-$n
[ -d "$dir/_work" ] && find "$dir/_work" -mindepth 1 -maxdepth 1 -exec rm -rf {} +
cd "$dir" || exit 1
exec ./run.sh

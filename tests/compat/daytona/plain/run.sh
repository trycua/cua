#!/usr/bin/env bash
# The plain form: `cua run --compat daytona -- python app.py`, no other flags,
# an unmodified Daytona program. It must create a sandbox of Daytona's default
# size (1 vCPU / 1 GiB), run a command in it and finish.
#
#   plain/run.sh /path/to/cua        (a daemon of its own in a scratch CUA_HOME)
set -euo pipefail
CUA="${1:?path to the cua binary}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/cua-daytona-plain.XXXXXX")"
export CUA_HOME="$WORK/home" DO_NOT_TRACK=1 CUA_TELEMETRY=0
mkdir -p "$CUA_HOME" "$WORK/job"
cp "$HERE/app.py" "$WORK/job/app.py"
"$CUA" daemon start --foreground >"$WORK/daemon.log" 2>&1 &
DAEMON=$!
cleanup() { "$CUA" daemon stop >/dev/null 2>&1 || true; kill "$DAEMON" 2>/dev/null || true; rm -rf "$WORK"; }
trap cleanup EXIT
for _ in $(seq 100); do "$CUA" daemon status >/dev/null 2>&1 && break; sleep 0.3; done
cd "$WORK/job"
git init -q . && git add -A && git -c user.name=t -c user.email=t@t commit -qm app
out="$("$CUA" run --compat daytona -- python app.py 2>&1)"
echo "$out"
grep -q "PLAIN-DONE" <<<"$out" || { echo "plain form did not finish" >&2; exit 1; }
grep -Eq "cpu 1 memory 1 " <<<"$out" || { echo "default size is not 1 vCPU / 1 GiB" >&2; exit 1; }
echo "plain form ok"

#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Runs a command and runs it again, with backoff, when it fails with an
# Apple-side error: the timestamp server ("The timestamp service is not
# available", "timestamp expected but not found"), the notary service
# answering 5xx or timing out, or a dropped connection. Any other failure
# (a bad identity, a rejected submission, a missing file) is final at once, so
# a real error is never hidden behind retries.
#
#   retry-apple.sh [--attempts N] [--delay SECONDS] [--stream] -- command args...
#
# Waits DELAY, 2*DELAY, 4*DELAY ... between attempts (default 4 attempts, 20 s).
# The command's stdout stays on stdout and its stderr on stderr, shown when it
# ends; --stream shows both live (merged) for a long command whose output is
# only a log. RETRY_APPLE_PATTERN overrides the pattern (an extended regex,
# matched case-insensitively).
set -uo pipefail

attempts=4
delay=20
stream=0
while [ $# -gt 0 ]; do
  case "$1" in
    --attempts) attempts="$2"; shift 2 ;;
    --delay) delay="$2"; shift 2 ;;
    --stream) stream=1; shift ;;
    --) shift; break ;;
    *) break ;;
  esac
done
[ $# -gt 0 ] || { echo "usage: retry-apple.sh [--attempts N] [--delay S] [--stream] -- command args..." >&2; exit 2; }

pattern="${RETRY_APPLE_PATTERN:-timestamp (expected but not found|service)|unable to (obtain|get) (a )?timestamp|(timed out|timeout)|network (connection|is unreachable)|connection (reset|refused|was lost|failed|closed)|could not connect|temporarily unavailable|service unavailable|internal server error|bad gateway|gateway time-?out|http( status)?( code)?:? ?5[0-9][0-9]|status code:? ?5[0-9][0-9]|ECONNRESET|ETIMEDOUT|ENOTFOUND|socket hang up|The operation couldn.t be completed}"

out="$(mktemp)"
err="$(mktemp)"
trap 'rm -f "$out" "$err"' EXIT

attempt=1
while :; do
  if [ "$stream" = 1 ]; then
    "$@" 2>&1 | tee "$out"
    rc="${PIPESTATUS[0]}"
    : >"$err"
  else
    "$@" >"$out" 2>"$err"
    rc=$?
    cat "$out"
    cat "$err" >&2
  fi
  [ "$rc" -eq 0 ] && exit 0
  if [ "$attempt" -ge "$attempts" ]; then
    echo "retry-apple: giving up after $attempt attempt(s) (exit $rc): $1" >&2
    exit "$rc"
  fi
  if ! cat "$out" "$err" | grep -Eiq -- "$pattern"; then
    exit "$rc"
  fi
  wait_s=$((delay * (1 << (attempt - 1))))
  echo "::warning::$1 failed with an Apple-side error (attempt $attempt of $attempts, exit $rc); retrying in ${wait_s}s" >&2
  sleep "$wait_s"
  attempt=$((attempt + 1))
done

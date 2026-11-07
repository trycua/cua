#!/usr/bin/env bash
# Source this file, then run a flaky network command as `retry gh release download ...`.
# Retries up to 5 times with a growing pause, so one GitHub 5xx does not fail a job.
retry() {
  local attempt=1 max=5
  until "$@"; do
    if [ "$attempt" -ge "$max" ]; then
      echo "retry: giving up after $max attempts: $*" >&2
      return 1
    fi
    echo "retry: attempt $attempt failed, retrying in $((attempt * 3))s: $*" >&2
    sleep $((attempt * 3))
    attempt=$((attempt + 1))
  done
}

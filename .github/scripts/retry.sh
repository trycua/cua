#!/usr/bin/env bash
# Source this file, then run a flaky network command as `retry gh release download ...`.
# Retries up to 5 times with a growing pause, so one GitHub 5xx does not fail a job.
#
# `gh release download` leaves the files it already fetched when a later asset
# fails, and refuses to overwrite them on the next attempt. So a retried
# download gets --clobber, unless the caller already chose how to handle
# existing files (--clobber, --skip-existing) or writes to one --output file.
retry() {
  local attempt=1 max=5
  local cmd=("$@")
  if [ "${1-}" = gh ] && [ "${2-}" = release ] && [ "${3-}" = download ]; then
    local arg needs_clobber=1
    for arg in "$@"; do
      case "$arg" in
        --clobber | --skip-existing | --output | --output=* | -O) needs_clobber=0 ;;
      esac
    done
    [ "$needs_clobber" -eq 1 ] && cmd+=(--clobber)
  fi
  until "${cmd[@]}"; do
    if [ "$attempt" -ge "$max" ]; then
      echo "retry: giving up after $max attempts: ${cmd[*]}" >&2
      return 1
    fi
    echo "retry: attempt $attempt failed, retrying in $((attempt * 3))s: ${cmd[*]}" >&2
    sleep $((attempt * 3))
    attempt=$((attempt + 1))
  done
}

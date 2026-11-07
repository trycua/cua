#!/bin/sh
# Install apt packages inside a container job or `docker run`, where the
# apt-install action (hosted runners only) cannot be used.
#   sh .github/scripts/apt-retry.sh PACKAGE...
# Each command is capped at 5 minutes and the pair is tried 3 times, so a slow
# or unreachable mirror ends in a clear error instead of hanging until the job
# timeout. Works in dash; runs apt-get directly as root, through sudo otherwise.
set -u
[ "$#" -gt 0 ] || { echo "usage: apt-retry.sh PACKAGE..." >&2; exit 2; }
if [ "$(id -u)" = 0 ]; then sudo=""; else sudo="sudo"; fi
export DEBIAN_FRONTEND=noninteractive
attempt=1
while :; do
  echo "apt-retry: attempt $attempt of 3 (limit 300s per command)"
  # shellcheck disable=SC2086 # $sudo is empty or one word
  if timeout 300 $sudo apt-get update -qq \
    && timeout 300 $sudo apt-get install -y -qq --no-install-recommends -o Acquire::Retries=3 "$@"; then
    exit 0
  fi
  echo "::warning::apt-retry attempt $attempt failed or timed out (slow or unreachable mirror)"
  [ "$attempt" -ge 3 ] && break
  sleep $((attempt * 10))
  attempt=$((attempt + 1))
done
echo "::error::apt-retry: no success after 3 attempts; the mirror is slow or unreachable, not a code failure. Re-run the job."
exit 1

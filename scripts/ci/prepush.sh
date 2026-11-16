#!/usr/bin/env bash
#
# Kept for old muscle memory and scripts: the local pre-push checks now live
# in bin/ci (see docs/contributing/CI.md). This maps the old flags onto it.
#
#   scripts/ci/prepush.sh              ->  bin/ci
#   scripts/ci/prepush.sh --fast       ->  bin/ci --quick
#   scripts/ci/prepush.sh --bindings   ->  bin/ci (bindings run when your change affects them)
#   scripts/ci/prepush.sh --base REF   ->  bin/ci --base REF
set -euo pipefail
args=()
while [ $# -gt 0 ]; do
  case "$1" in
    --fast) args+=(--quick) ;;
    --bindings) ;;
    --base) args+=(--base "$2"); shift ;;
    -h|--help) sed -n '3,9p' "$0"; exit 0 ;;
    *) args+=("$1") ;;
  esac
  shift
done
exec "$(git rev-parse --show-toplevel)/bin/ci" ${args[@]+"${args[@]}"}

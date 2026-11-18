#!/usr/bin/env bash
# Run the pinned Release Please CLI with `additional-paths` support
# (release_please_additional_paths.cjs). Arguments go to the CLI unchanged:
#
#   .github/scripts/release_please.sh release-pr --token=... --repo-url=trycua/cua ...
set -euo pipefail

ENGINE_VERSION=17.3.0
EXPECTED_INTEGRITY='sha512-dB7HsFUpAvU1Wj9RGCINUz8Zi2qQDZiGbDEEVnJ7baNfJmk5XdYhuUFL6RcuDGjReGhp1gzY8Tc2g7vHlM4zpg=='
ACTUAL_INTEGRITY=$(npm view "release-please@$ENGINE_VERSION" dist.integrity)
if [[ "$ACTUAL_INTEGRITY" != "$EXPECTED_INTEGRITY" ]]; then
  echo "::error::release-please@$ENGINE_VERSION integrity mismatch"
  exit 1
fi

ENGINE="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/release-please-cli"
NPM_CONFIG_IGNORE_SCRIPTS=true npm install --prefix "$ENGINE" --ignore-scripts --no-audit --no-fund \
  "release-please@$ENGINE_VERSION" > /dev/null
SCRIPTS=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
NODE_PATH="$ENGINE/node_modules" exec node --require "$SCRIPTS/release_please_additional_paths.cjs" \
  "$ENGINE/node_modules/release-please/build/src/bin/release-please.js" "$@"

#!/usr/bin/env bash
# Builds `cua-test-fixtures` (the loopback fixture server behind the language
# binding smoke tests) into libs/cua/target/<profile>/, where the Python,
# TypeScript, Swift and Kotlin tests look for it.
#
# The binary lives in libs/cua-spacesd/tests/spaces-e2e because it links a
# real cua-spacesd server core; libs/cua itself never depends on the driver
# workspace.
#
#   libs/cua/scripts/build-test-fixtures.sh            # debug
#   libs/cua/scripts/build-test-fixtures.sh --release  # extra cargo args pass through
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
cua="$(cd "$here/.." && pwd)"
exec cargo build --locked \
  --manifest-path "$cua/../cua-spacesd/tests/spaces-e2e/Cargo.toml" \
  --target-dir "$cua/target" \
  --bin cua-test-fixtures "$@"

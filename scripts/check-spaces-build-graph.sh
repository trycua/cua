#!/usr/bin/env bash
# Guard for the Cua Spaces Local build time (CUA-3971): cua-compat (the E2B,
# Daytona and Modal gateway, 47k lines) is built beside cua-cli, not before it.
# The Cua Spaces `cua` (cua-spaces-cli) takes cua-cli without its `compat`
# feature and carries the gateway itself, so the two compile in parallel.
# Cargo unifies features, so one dependency that asks for cua-cli's default
# features would quietly put the gateway back in front of the CLI.
#
# cua-ci (the CI runner pool) does not depend on cua-sdk either: its one SDK
# file (SdkSpaces) lives in cua-cli, so cua-ci builds beside cua-sdk, not after it.
#
# Fails when, in the graph `install-local.sh` builds (`--no-default-features
# -p cua-spaces-ffi -p cua-spaces-cli`), cua-cli has its `compat` feature or
# anything but cua-spaces-cli depends on cua-compat.
#
#   scripts/check-spaces-build-graph.sh
set -euo pipefail
cd "$(git rev-parse --show-toplevel)/libs/cua"

args=(--locked --no-default-features -p cua-spaces-ffi -p cua-spaces-cli)

features=$(cargo tree "${args[@]}" -e features -i cua-cli)
if grep -q 'cua-cli feature "compat"' <<<"$features"; then
  echo "check-spaces-build-graph: cua-cli's \`compat\` feature is on in the Cua Spaces build." >&2
  echo "  Something depends on cua-cli with default features; the gateway builds before the CLI again." >&2
  echo "$features" >&2
  exit 1
fi

dependents=$(cargo tree "${args[@]}" -e normal -i cua-compat --depth 1)
users=$(grep -E '^[├└]' <<<"$dependents" | grep -v 'cua-spaces-cli' || true)
if [ -n "$users" ]; then
  echo "check-spaces-build-graph: cua-compat has dependents besides cua-spaces-cli:" >&2
  echo "$users" >&2
  exit 1
fi
if cargo tree --locked -p cua-ci -e normal --prefix none --depth 1 | grep -q "^cua-sdk v"; then
  echo "check-spaces-build-graph: cua-ci depends on cua-sdk; keep SdkSpaces in cua-cli." >&2
  exit 1
fi
echo "check-spaces-build-graph: ok"

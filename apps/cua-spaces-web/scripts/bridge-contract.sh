#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# The bridge contract on every host, locally (CI runs the same steps):
#  1. web: the shapes document is current, every operation has a shape, the
#     coverage table matches the Swift method list, the demo host answers in
#     shape (vitest);
#  2. Electron: the method table matches the Swift list and this contract,
#     and (with `pnpm native` built there) the host's answers on the app core
#     are in shape and read through the Electron adapter (vitest in
#     apps/cua-spaces-desktop);
#  3. SwiftUI: every listed method is routed and every routed one listed,
#     and the answers on fixture backends are in shape (swift test); those
#     answers then go through the webkit adapter (vitest, CUA_BRIDGE_ANSWERS);
#  4. with --electron: the built Electron shell, through its preload, plus
#     the parity flows on it (pnpm parity:electron).
# --no-swift skips 3 (no Swift toolchain, or the app library isn't staged).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
web=$PWD
desktop=$web/../cua-spaces-desktop
macos=$web/../cua-spaces-macos
# No git credential prompts from the builds (see the macOS app's README).
export GIT_TERMINAL_PROMPT=0 GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=credential.helper GIT_CONFIG_VALUE_0=

swift=1 electron=0
for a in "$@"; do
  case $a in
    --no-swift) swift=0 ;;
    --electron) electron=1 ;;
    --) ;; # `pnpm contract -- --electron`: pnpm 11 passes the separator on
    *) echo "usage: $0 [--no-swift] [--electron]" >&2; exit 2 ;;
  esac
done

echo "== web: shapes, coverage, the demo host"
npx vitest run src/bridge/__tests__/contract.test.ts src/bridge/__tests__/coverage.test.ts

echo "== Electron: the method table, and its answers on the app core"
(cd "$desktop" && npx vitest run test/bridge-registry.test.ts test/bridge-native.test.ts)

if [[ $swift == 1 ]]; then
  answers=$(mktemp -t cua-bridge-answers).json
  trap 'rm -f "$answers"' EXIT
  echo "== SwiftUI: routing and shapes on fixtures"
  (cd "$macos" && CUA_BRIDGE_ANSWERS=$answers scripts/test.sh --filter BridgeContractTests)
  echo "== SwiftUI: its answers through the webkit adapter"
  CUA_BRIDGE_ANSWERS=$answers npx vitest run src/bridge/__tests__/contract.test.ts
fi

if [[ $electron == 1 ]]; then
  echo "== Electron shell: the preload, and the parity flows"
  pnpm parity:electron
fi
echo "bridge contract: ok"

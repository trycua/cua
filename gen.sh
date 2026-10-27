#!/bin/bash
# Temporary (not committed): regenerate bindings, the web core wasm and docs on the builder.
set -u
R=$PWD
echo "== uniffi"; (npm --prefix libs/cua/typescript ci --ignore-scripts >/dev/null 2>&1 && node libs/cua/scripts/generate-uniffi-bindings.mjs 2>&1 | tail -5) ; echo "uniffi exit $?"
echo "== spaces-app-swift"; node libs/spaces-app-swift/scripts/generate-bindings.mjs 2>&1 | tail -5
echo "== electron"; (cd apps/cua-spaces-desktop && (corepack enable >/dev/null 2>&1; pnpm install --frozen-lockfile >/dev/null 2>&1) && pnpm native -- --bindings 2>&1 | tail -5)
echo "== wasm"; (cd apps/cua-spaces-web && node scripts/build-core-wasm.mjs 2>&1 | tail -5)
echo "== docs"; (cd docs && pnpm install --frozen-lockfile >/dev/null 2>&1; pnpm docs:generate:cua-cli 2>&1 | tail -5; pnpm docs:generate:cua-sdk 2>&1 | tail -5; pnpm docs:check 2>&1 | tail -20)
tar czf "$R/libs/cua/gen.tgz" --exclude=node_modules --exclude='*.so' --exclude='*.dylib' --exclude='target' \
  libs/cua/swift libs/cua/typescript/src libs/cua/kotlin libs/cua/python libs/spaces-app-swift \
  apps/cua-spaces-desktop/src/native/generated apps/cua-spaces-web/src/bridge/core/wasm docs/content scripts/docs-generators/cli-specs 2>/dev/null
ls -la "$R/libs/cua/gen.tgz"

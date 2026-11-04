#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Prepare a live Cua Spaces session: make sure the shared `cua daemon` is
# running, then print (or, with --launch, run) the command that starts the app.
#
#   ./run-live.sh            # start `cua daemon`, print the launch command
#   ./run-live.sh --launch   # ...and exec `pnpm tauri dev` (opens the app's windows)
#
# Local, "Add by address", relay and your own machines' Spaces need no
# credentials here: sign in from the app (Settings -> Account) or with
# `cua auth login`. Cua Cloud has ended.
#
# The daemon is shared: the app, the `cua` CLI and `cua daemon mcp` (Claude,
# Codex, OpenClaw) all see the same Spaces registry (~/.cua/spaces.json).
#
# The cua binary: $CUA_BIN, else `cua` on PATH, else built from this repo with
# cargo (libs/cua, package cua-cli).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
LAUNCH=0
[ "${1:-}" = "--launch" ] && LAUNCH=1

# Resolve the cua CLI.
cua_cmd=()
if [ -n "${CUA_BIN:-}" ]; then
  cua_cmd=("$CUA_BIN")
elif command -v cua >/dev/null 2>&1; then
  cua_cmd=("$(command -v cua)")
else
  echo "==> no cua on PATH; building cua-cli from $REPO/libs/cua (release)"
  cargo build --manifest-path "$REPO/libs/cua/Cargo.toml" -p cua-cli --release
  cua_cmd=("$REPO/libs/cua/target/release/cua")
fi
export CUA_BIN="${cua_cmd[0]}"

# `cua daemon start` backgrounds itself and is a no-op when one is running.
echo "==> Ensuring the cua daemon is running ($CUA_BIN)"
"${cua_cmd[@]}" daemon start
"${cua_cmd[@]}" daemon status || true

if [ "$LAUNCH" = 1 ]; then
  cd "$HERE"
  exec pnpm tauri dev
fi

cat <<EOF

The daemon is up. To start the app (this opens its windows on your desktop):

    cd $HERE && ./run-live.sh --launch

or directly:

    cd $HERE && CUA_BIN=$CUA_BIN pnpm tauri dev

Register the Spaces MCP with your agent CLIs: Settings -> Agents in the app, or
    claude mcp add cua-spaces -- $CUA_BIN daemon mcp
    codex mcp add cua-spaces -- $CUA_BIN daemon mcp
EOF

#!/bin/bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# install-agent-cli.sh — install the coding-agent CLI that runs inside a Space.
#
# A Space is only useful if an agent can actually run in it. The CLI is the
# binary; the *credentials* arrive separately by teleport (`teleport_app
# claude-code`), so nothing here logs in and nothing identity-bearing is baked.
#
# Without this the image looks complete and then fails at the last moment with
#   run-agent.sh: line 7: exec: claude: not found
# which is only visible once you try a real run.
set -uo pipefail

say()  { printf '\033[1;36m==> %s\033[0m\n' "$*"; }
fail() { echo "$*" >&2; exit 1; }

BIN="$HOME/.local/bin"
mkdir -p "$BIN"
export PATH="$BIN:$HOME/.local/node/bin:$PATH"

if command -v claude >/dev/null 2>&1; then
  say "Claude Code already installed: $(claude --version 2>/dev/null | head -1)"
  exit 0
fi

say "Installing Claude Code (official installer)"
# Installs a self-contained build into ~/.local/bin/claude. No sudo, no Node
# needed, and nothing written outside $HOME.
if curl -fsSL https://claude.ai/install.sh | bash; then
  :
else
  say "Official installer failed; falling back to npm"
  command -v npm >/dev/null 2>&1 || fail "npm unavailable and the official installer failed"
  npm install -g @anthropic-ai/claude-code || fail "npm install failed"
fi

hash -r 2>/dev/null || true
command -v claude >/dev/null 2>&1 || fail "claude still not on PATH after install"

say "Installed: $(claude --version 2>/dev/null | head -1)"
echo "  path: $(command -v claude)"
echo
echo "Credentials are NOT baked: teleport them at runtime with"
echo "  teleport_app <space> claude-code"

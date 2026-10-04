#!/bin/bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# install-unity-cli.sh — put the official Unity CLI on the GUI PATH and make it
# the image's Unity MCP server.
#
# WHY THE CLI AND NOT THE EDITOR PACKAGE. The demo project pulls MCP for Unity
# from `https://github.com/CoplayDev/unity-mcp.git?path=/MCPForUnity#main` — a
# third-party package, tracking a moving branch, resolved from the *cloned
# project's* manifest. The golden ships no project, so at build time we cannot
# know which version a run will use, and a `mcpforunityserver` version pinned in
# the image would drift out of match with it. Driving that package also costs a
# manual **Connect** click ("No Session"), a two-step MCP Setup wizard and a
# "Client Configuration" dialog. Unity's own CLI replaces the in-Editor server
# and is the supported path:
#
#   * `unity mcp` is a **stdio** MCP server. No port, so no port discovery —
#     `~/.unity-mcp/unity-mcp-status-*.json` and `unity-mcp-port.json` do not
#     exist on this path and never will. (An earlier run spent ~42 minutes
#     polling for a file that was never coming, on guidance from our own docs.)
#   * `unity mcp --project-path <p>` targets one project, which a login-time
#     service could never do — the golden has no project until a run clones one.
#   * It ships with the Editor toolchain rather than resolving from the
#     project, so there is no `#main` skew.
#
# The CLI is installed by Unity Hub itself on first run, into ~/.unity/bin and
# recorded in the Hub's cli-install.json — but only on its first *GUI* run. A
# from-scratch build never gives the Hub one: install-unity.sh drives it
# headlessly, so on a bare VM ~/.unity/bin does not exist and this step used to
# fail the whole build with "Unity CLI not found". The binary is not missing,
# only unplaced: the Hub ships it inside its own bundle at
# Contents/Resources/cli/unity (same 1.0.0-beta.8 build it would have copied),
# so when the Hub has not done the copy we do exactly that copy ourselves.
# Then, as before, fix the one thing the Hub does not: the CLI is not on the
# PATH a GUI-launched app gets.
set -uo pipefail

CLI="$HOME/.unity/bin/unity"
DEST=/usr/local/bin
HUB_CLI_STATE="$HOME/Library/Application Support/UnityHub/cli-install.json"
# Recorded so a drift is visible in the build log.
EXPECTED_VERSION="1.0.0-beta.8"

say() { printf '\033[1;36m==> %s\033[0m\n' "$*"; }
sudo_run() { printf '%s\n' "${CUA_SUDO_PW:-lume}" | sudo -S -p '' "$@"; }
SUDO=""; [ -w "$DEST" ] || SUDO="sudo_run"

HUB_BUNDLED_CLI="/Applications/Unity Hub.app/Contents/Resources/cli/unity"

if [ ! -x "$CLI" ]; then
  # The Hub only performs this copy on a first GUI run, which a headless build
  # never gives it. Do it here rather than failing: same binary, same location,
  # same layout the Hub would have produced.
  if [ -x "$HUB_BUNDLED_CLI" ]; then
    say "Hub never GUI-launched, so it never placed the CLI — installing it from the Hub bundle"
    mkdir -p "$(dirname "$CLI")"
    cp "$HUB_BUNDLED_CLI" "$CLI"
    chmod 755 "$CLI"
    mkdir -p "$(dirname "$HUB_CLI_STATE")"
    printf '{"layout":"home","path":"%s","installedBy":"install-unity-cli.sh"}\n' \
      "$CLI" > "$HUB_CLI_STATE"
  else
    echo "Unity CLI not found at $CLI, and the Hub bundle has no copy at" >&2
    echo "$HUB_BUNDLED_CLI either. Make sure install-unity.sh ran and the Hub" >&2
    echo "was installed." >&2
    exit 1
  fi
fi

VERSION="$("$CLI" --version 2>/dev/null | tail -1 | tr -d '[:space:]')"
say "Unity CLI $VERSION at $CLI"
[ -f "$HUB_CLI_STATE" ] && sed 's/^/  /' "$HUB_CLI_STATE"
if [ "$VERSION" != "$EXPECTED_VERSION" ]; then
  echo "  note: expected $EXPECTED_VERSION; update EXPECTED_VERSION and the doc if this is intended"
fi

# A GUI app launched via `open` inherits PATH=/usr/bin:/bin:/usr/sbin:/sbin, so
# a shell profile cannot help — the same class of bug as the Python 3.9.6 one
# seed-python-for-unity-mcp.sh fixes. /usr/local/bin is on that PATH.
say "Linking into $DEST so GUI-launched processes can find it"
$SUDO mkdir -p "$DEST"
$SUDO ln -sf "$CLI" "$DEST/unity"
"$DEST/unity" --version >/dev/null 2>&1 \
  || { echo "$DEST/unity is not runnable" >&2; exit 1; }

# Quiet by default everywhere, including for anything the agent shells out to.
# The banner is cosmetic noise in a recorded demo, and an interactive prompt in
# a Space is unanswerable.
say "Recording non-interactive/quiet defaults"
mkdir -p "$HOME/.cua"
cat > "$HOME/.cua/unity-cli.env" <<'ENV'
UNITY_NO_BANNER=1
UNITY_NON_INTERACTIVE=1
UNITY_NO_PAGER=1
ENV

say "Verifying the MCP server starts and speaks stdio"
# `unity mcp` blocks waiting for a client, so give it a moment and check that it
# announced itself rather than erroring out.
UNITY_NO_BANNER=1 UNITY_NON_INTERACTIVE=1 "$DEST/unity" mcp </dev/null >/tmp/unity-mcp-probe.out 2>&1 &
probe=$!
sleep 6
kill "$probe" 2>/dev/null
if grep -q "MCP server started" /tmp/unity-mcp-probe.out; then
  sed 's/^/  /' /tmp/unity-mcp-probe.out
  say "Unity CLI MCP server OK"
else
  echo "unity mcp did not start:" >&2; cat /tmp/unity-mcp-probe.out >&2; exit 1
fi
rm -f /tmp/unity-mcp-probe.out

cat <<EOF

The agent gets this as its "unity" MCP server (see write-agent-mcp.sh):

  "unity": {"command": "$DEST/unity", "args": ["mcp"]}

which is exactly what \`unity mcp configure <client>\` writes. To pin it to one
project, add --project-path <path>; \`unity mcp\` on its own locates the running
Editor.
EOF

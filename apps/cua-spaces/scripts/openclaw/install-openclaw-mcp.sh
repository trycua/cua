#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Register the Cua Spaces MCP (`cua daemon mcp`, stdio) with a local OpenClaw
# Gateway. It shares the Spaces registry (~/.cua/spaces.json) with the Cua Spaces
# app and the cua CLI.
#
# Idempotent: re-running rewrites only the `mcp.servers["cua-spaces"]` entry and
# leaves the rest of ~/.openclaw/openclaw.json untouched. Never starts, installs
# or restarts a Gateway — it only edits config and tells you to restart.
#
#   ./install-openclaw-mcp.sh              # register (or update) the server
#   ./install-openclaw-mcp.sh --print      # print the JSON fragment, write nothing
#   ./install-openclaw-mcp.sh --remove     # unregister
#
# Env overrides:
#   OPENCLAW_CONFIG   config path (default ~/.openclaw/openclaw.json)
#   SERVER_NAME       MCP server key (default cua-spaces)
set -euo pipefail

SERVER_NAME="${SERVER_NAME:-cua-spaces}"
CONFIG="${OPENCLAW_CONFIG:-$HOME/.openclaw/openclaw.json}"
# --- resolve an absolute python3 (only used to edit JSON here) ---------------
PY=""
for cand in /opt/homebrew/bin/python3 /usr/local/bin/python3 "$(command -v python3 || true)" /usr/bin/python3; do
  [ -n "$cand" ] && [ -x "$cand" ] && { PY="$cand"; break; }
done
[ -n "$PY" ] || { echo "error: no python3 found" >&2; exit 1; }

# --- resolve the `cua` CLI ----------------------------------------------------
# The Spaces MCP is `cua daemon mcp` (the Rust cua CLI). A Gateway installed with
# `openclaw onboard --install-daemon` runs under launchd with
# PATH=/usr/bin:/bin:/usr/sbin:/sbin, so bake an absolute path.
CUA="${CUA_BIN:-}"
if [ -z "$CUA" ]; then
  for cand in "$(command -v cua 2>/dev/null || true)" "$HOME/.local/bin/cua" \
              /opt/homebrew/bin/cua /usr/local/bin/cua "$HOME/.cargo/bin/cua"; do
    [ -n "$cand" ] && [ -x "$cand" ] && { CUA="$cand"; break; }
  done
fi
[ -n "$CUA" ] || { echo "error: the cua CLI was not found (set CUA_BIN or install it)" >&2; exit 1; }

# --- build the fragment -----------------------------------------------------
FRAGMENT="$(
  CUA="$CUA" SERVER_NAME="$SERVER_NAME" HOME_DIR="$HOME" "$PY" - <<'PYEOF'
import json, os
entry = {
    "command": os.environ["CUA"],
    "args": ["daemon", "mcp"],
    "transport": "stdio",
    "enabled": True,
    # Claiming or provisioning a Space waits for it to boot; teleport moves a
    # whole app profile. Both are far slower than a default MCP request timeout.
    "connectionTimeoutMs": 20000,
    "requestTimeoutMs": 900000,
    "env": {"HOME": os.environ["HOME_DIR"]},
}
print(json.dumps({"mcp": {"servers": {os.environ["SERVER_NAME"]: entry}}}, indent=2))
PYEOF
)"

if [ "${1:-}" = "--print" ]; then echo "$FRAGMENT"; exit 0; fi

# --- merge into the config --------------------------------------------------
mkdir -p "$(dirname "$CONFIG")"
[ -f "$CONFIG" ] || echo '{}' > "$CONFIG"

# Validate what is already there before we touch it, so we never clobber a
# hand-edited config that happens to be broken.
"$PY" -c 'import json,sys; json.load(open(sys.argv[1]))' "$CONFIG" 2>/dev/null \
  || { echo "error: $CONFIG is not valid JSON — fix or move it first" >&2; exit 1; }

cp "$CONFIG" "$CONFIG.bak"

MODE="add"; [ "${1:-}" = "--remove" ] && MODE="remove"

CONFIG="$CONFIG" FRAGMENT="$FRAGMENT" SERVER_NAME="$SERVER_NAME" MODE="$MODE" "$PY" - <<'PYEOF'
import json, os
path, name, mode = os.environ["CONFIG"], os.environ["SERVER_NAME"], os.environ["MODE"]
with open(path) as fh:
    cfg = json.load(fh)
servers = cfg.setdefault("mcp", {}).setdefault("servers", {})
if mode == "remove":
    removed = servers.pop(name, None)
    print(f"{'removed' if removed else 'not present:'} {name}")
else:
    entry = json.loads(os.environ["FRAGMENT"])["mcp"]["servers"][name]
    action = "updated" if name in servers else "registered"
    servers[name] = entry
    print(f"{action} mcp.servers[{name!r}]")
with open(path, "w") as fh:
    json.dump(cfg, fh, indent=2)
    fh.write("\n")
PYEOF

echo
echo "config: $CONFIG  (backup: $CONFIG.bak)"
echo "server: $CUA daemon mcp"
echo
echo "NEXT: restart the Gateway so it picks up the MCP change:"
echo "    openclaw gateway restart     # changes to mcp.servers are read at startup"
echo "Then verify the server connected:"
echo "    openclaw mcp list"

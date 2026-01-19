#!/usr/bin/env bash
set -Eeuo pipefail

# --- Resolve repo root from this script's location ---
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )"
CUA_REPO_DIR="$( cd "$SCRIPT_DIR/../../../.." &> /dev/null && pwd )"

# --- Choose a Python interpreter (prefer repo-root venv) ---
CANDIDATES=(
  "$CUA_REPO_DIR/.venv/bin/python"
  "$CUA_REPO_DIR/libs/.venv/bin/python"
  "$(command -v python3 || true)"
  "$(command -v python || true)"
)

PYTHON_PATH=""
for p in "${CANDIDATES[@]}"; do
  if [[ -n "$p" && -x "$p" ]]; then
    PYTHON_PATH="$p"
    break
  fi
done

if [[ -z "${PYTHON_PATH}" ]]; then
  >&2 echo "[cua-mcp] ERROR: No suitable Python found. Tried:"
  for p in "${CANDIDATES[@]}"; do >&2 echo "  - $p"; done
  >&2 echo "[cua-mcp] Tip: create venv:  python3 -m venv $CUA_REPO_DIR/.venv && \"$CUA_REPO_DIR/.venv/bin/pip\" install -e \"$CUA_REPO_DIR/libs/python/mcp-server\""
  exit 127
fi

# --- Export PYTHONPATH so module imports work during dev ---
export PYTHONPATH="$CUA_REPO_DIR/libs/python/mcp-server:$CUA_REPO_DIR/libs/python/agent:$CUA_REPO_DIR/libs/python/computer:$CUA_REPO_DIR/libs/python/core"

# --- Helpful startup log for Claude's mcp.log ---
>&2 echo "[cua-mcp] using python: $PYTHON_PATH"
>&2 echo "[cua-mcp] repo dir    : $CUA_REPO_DIR"
>&2 echo "[cua-mcp] PYTHONPATH  : $PYTHONPATH"
if [[ -n "${CUA_MODEL_NAME:-}" ]]; then
  >&2 echo "[cua-mcp] CUA_MODEL_NAME=$CUA_MODEL_NAME"
fi

# --- Run the MCP server module ---
exec "$PYTHON_PATH" -m mcp_server.server

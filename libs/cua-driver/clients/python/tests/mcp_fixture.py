"""Deterministic executable MCP peer used by transport contract tests."""

from __future__ import annotations

import json
import sys
from typing import Any


initialized = False


def respond(request: dict[str, Any]) -> None:
    global initialized
    method = request.get("method")
    if method == "notifications/initialized":
        initialized = True
        return
    request_id = request.get("id")
    if method == "initialize":
        result: dict[str, Any] = {
            "protocolVersion": request["params"]["protocolVersion"],
            "capabilities": {},
            "serverInfo": {"name": "fixture", "version": "1"},
        }
    elif not initialized:
        payload = {
            "jsonrpc": "2.0",
            "id": request_id,
            "error": {"code": -32002, "message": "not initialized"},
        }
        print(json.dumps(payload, separators=(",", ":")), flush=True)
        return
    elif method == "test/hang":
        return
    elif method == "tools/list":
        result = {"tools": [{"name": "get_desktop_state"}]}
    elif method == "tools/call":
        params = request.get("params", {})
        result = {
            "content": [{"type": "text", "text": "fixture result"}],
            "structuredContent": {
                "name": params.get("name"),
                "arguments": params.get("arguments", {}),
            },
        }
    else:
        payload = {
            "jsonrpc": "2.0",
            "id": request_id,
            "error": {"code": -32601, "message": f"unknown method: {method}"},
        }
        print(json.dumps(payload, separators=(",", ":")), flush=True)
        return
    payload = {"jsonrpc": "2.0", "id": request_id, "result": result}
    print(json.dumps(payload, separators=(",", ":")), flush=True)


for line in sys.stdin:
    value = json.loads(line)
    if isinstance(value, dict):
        respond(value)

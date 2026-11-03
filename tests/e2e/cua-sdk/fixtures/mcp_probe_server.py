"""A stdlib-only MCP-style JSON-RPC server for the unified-sandbox-api e2e.

Run as the sandbox command (`python -c <this file>`) in a plain
`python:3.12-slim` image. Like a streamable-HTTP MCP server it refuses
requests whose `accept` header lacks `application/json` and
`text/event-stream` (400), hands out an `mcp-session-id` on `initialize` and
requires it afterwards, so a client that drops headers fails loudly.
"""

import json
import os
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(os.environ.get("MCP_PORT", "8765"))
SESSIONS = set()


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def _send(self, status, body, headers=()):
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        for k, v in headers:
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path.startswith("/health"):
            self._send(200, {"ok": True})
        else:
            self._send(404, {"error": "not found"})

    def do_POST(self):
        if not self.path.startswith("/mcp"):
            return self._send(404, {"error": "not found"})
        accept = self.headers.get("accept", "")
        if "application/json" not in accept or "text/event-stream" not in accept:
            return self._send(
                400, {"error": "accept must list application/json and text/event-stream"}
            )
        req = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))) or b"{}")
        method, rid = req.get("method"), req.get("id")
        if method == "initialize":
            sid = uuid.uuid4().hex
            SESSIONS.add(sid)
            result = {
                "protocolVersion": "2025-03-26",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "cua-e2e-mcp", "version": "1"},
            }
            return self._send(
                200, {"jsonrpc": "2.0", "id": rid, "result": result}, [("mcp-session-id", sid)]
            )
        if self.headers.get("mcp-session-id") not in SESSIONS:
            return self._send(400, {"error": "missing or unknown mcp-session-id"})
        if method == "tools/list":
            result = {
                "tools": [
                    {"name": "add", "inputSchema": {"type": "object"}},
                    {"name": "env", "inputSchema": {"type": "object"}},
                ]
            }
        elif method == "tools/call":
            p = req.get("params") or {}
            a = p.get("arguments") or {}
            if p.get("name") == "add":
                text = str(a["a"] + a["b"])
            else:
                text = os.environ.get(a.get("var", "GREETING"), "")
            result = {"content": [{"type": "text", "text": text}]}
        else:
            result = {}
        self._send(200, {"jsonrpc": "2.0", "id": rid, "result": result})


ThreadingHTTPServer(("0.0.0.0", PORT), Handler).serve_forever()

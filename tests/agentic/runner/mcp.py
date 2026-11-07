"""A small MCP client for `cua-driver mcp` (line-delimited JSON-RPC on stdio),
used by the runner's own session: dialog watching, ui checks and replay. The
agent has its own session through proxy.py."""

from __future__ import annotations

import json
import queue
import re
import threading
import time
from typing import Any

PROTOCOL = "2025-06-18"


class McpError(RuntimeError):
    pass


class ToolResult:
    def __init__(self, message: dict):
        result = message.get("result") or {}
        error = message.get("error")
        self.is_error = bool(error) or bool(result.get("isError"))
        self.structured: dict = result.get("structuredContent") or {}
        self.content: list = result.get("content") or []
        self.text = "\n".join(p.get("text", "") for p in self.content if p.get("type") == "text")
        if error:
            self.text = json.dumps(error)

    def first_line(self) -> str:
        return (self.text.strip().splitlines() or [""])[0][:300]


class Mcp:
    def __init__(self, proc, name: str = "agentic-e2e"):
        self.proc = proc
        self.lines: queue.Queue = queue.Queue()
        self.lock = threading.Lock()
        self.next_id = 1
        threading.Thread(target=self._read, daemon=True).start()
        r = self.request(
            "initialize",
            {
                "protocolVersion": PROTOCOL,
                "capabilities": {},
                "clientInfo": {"name": name, "version": "1"},
            },
            timeout=60,
        )
        if "result" not in r:
            raise McpError(f"initialize failed: {r}")
        self._send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def _read(self):
        for line in self.proc.stdout:
            self.lines.put(line)
        self.lines.put(None)

    def _send(self, msg: dict) -> None:
        self.proc.stdin.write(json.dumps(msg) + "\n")
        self.proc.stdin.flush()

    def request(self, method: str, params: dict, timeout: float = 120) -> dict:
        with self.lock:
            rid = self.next_id
            self.next_id += 1
            self._send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
            deadline = time.monotonic() + timeout
            while True:
                left = deadline - time.monotonic()
                if left <= 0:
                    raise McpError(f"{method} timed out after {timeout}s")
                try:
                    line = self.lines.get(timeout=left)
                except queue.Empty:
                    continue
                if line is None:
                    raise McpError("cua-driver mcp exited")
                try:
                    msg = json.loads(line)
                except ValueError:
                    continue
                if msg.get("id") == rid:
                    return msg

    def call(self, tool: str, args: dict[str, Any], timeout: float = 120) -> ToolResult:
        return ToolResult(
            self.request("tools/call", {"name": tool, "arguments": args}, timeout=timeout)
        )

    def close(self) -> None:
        try:
            self.proc.stdin.close()
            self.proc.wait(timeout=10)
        except Exception:
            try:
                self.proc.kill()
            except Exception:
                pass


ROW = re.compile(r"^\s*-\s*\[(\d+)\]\s*(.*)$")


def rows(tree_markdown: str):
    for line in tree_markdown.splitlines():
        m = ROW.match(line)
        if m:
            yield int(m.group(1)), m.group(2)


def tree_text(r: ToolResult) -> str:
    return r.structured.get("tree_markdown") or r.text


def windows(mcp: Mcp, app: str | None = None) -> list[dict]:
    args = {"app": app} if app else {}
    r = mcp.call("list_windows", args, timeout=60)
    if r.is_error:
        return []
    ws = r.structured.get("windows")
    if isinstance(ws, list):
        return ws
    return []


def window_title(w: dict) -> str:
    return str(w.get("title") or w.get("name") or "")


def window_app(w: dict) -> str:
    return str(w.get("app_name") or w.get("app") or w.get("owner") or "")

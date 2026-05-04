"""Low-level MCP stdio client for cua-driver.

Speaks JSON-RPC 2.0 line-framed over stdin/stdout. Async-safe read pump
runs on a background thread; the public call_tool() API is synchronous.
"""

from __future__ import annotations

import json
import os
import subprocess
import threading
import time
from collections import deque
from typing import Any, Optional


def default_binary_path() -> str:
    """Return the cua-driver binary path.

    Resolution order:
    1. ``CUA_DRIVER_BINARY`` environment variable.
    2. ``<repo-root>/.build/debug/cua-driver`` (Swift debug build).
    """
    return os.environ.get(
        "CUA_DRIVER_BINARY",
        os.path.normpath(
            os.path.join(
                os.path.dirname(os.path.abspath(__file__)),
                "..", "..", "..", "..", ".build", "debug", "cua-driver",
            )
        ),
    )


class MCPCallError(RuntimeError):
    """Raised when the MCP server returns an error response."""

    def __init__(self, error: dict) -> None:
        super().__init__(f"MCP error {error.get('code')}: {error.get('message')}")
        self.code = error.get("code")
        self.message = error.get("message")


class DriverClient:
    """Synchronous MCP stdio client.

    Use as a context manager::

        with DriverClient() as c:
            result = c.call_tool("list_apps")

    Or manage lifetime manually::

        c = DriverClient()
        c.start()
        try:
            result = c.call_tool("list_apps")
        finally:
            c.stop()
    """

    def __init__(
        self,
        binary_path: Optional[str] = None,
        subcommand: str = "mcp",
    ) -> None:
        self.binary_path = binary_path or default_binary_path()
        self.subcommand = subcommand
        self._process: Optional[subprocess.Popen] = None
        self._next_id = 0
        self._lines: deque[str] = deque()
        self._reader: Optional[threading.Thread] = None
        self._lock = threading.Lock()

    # ------------------------------------------------------------------
    # Context manager / lifecycle
    # ------------------------------------------------------------------

    def __enter__(self) -> "DriverClient":
        self.start()
        return self

    def __exit__(self, *exc: Any) -> None:
        self.stop()

    def start(self) -> None:
        """Launch cua-driver and perform the MCP handshake."""
        self._process = subprocess.Popen(
            [self.binary_path, self.subcommand],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
        )
        self._reader = threading.Thread(target=self._pump_stdout, daemon=True)
        self._reader.start()
        self._handshake()

    def stop(self) -> None:
        """Terminate the cua-driver process."""
        if not self._process:
            return
        for stream in (self._process.stdin, self._process.stdout, self._process.stderr):
            try:
                if stream is not None:
                    stream.close()
            except Exception:
                pass
        try:
            self._process.terminate()
            self._process.wait(timeout=3)
        except Exception:
            self._process.kill()
            self._process.wait(timeout=1)
        self._process = None

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def list_tools(self) -> list[dict]:
        """Return the list of tools the server exposes."""
        return self._call("tools/list")["tools"]

    def call_tool(self, name: str, arguments: Optional[dict] = None, timeout: float = 30.0) -> dict:
        """Call a named MCP tool with the given arguments."""
        return self._call(
            "tools/call",
            {"name": name, "arguments": arguments or {}},
            timeout=timeout,
        )

    # ------------------------------------------------------------------
    # Internal
    # ------------------------------------------------------------------

    def _pump_stdout(self) -> None:
        assert self._process and self._process.stdout
        for line in self._process.stdout:
            line = line.strip()
            if line:
                self._lines.append(line)

    def _handshake(self) -> None:
        self._call(
            "initialize",
            {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "cua-driver-sdk", "version": "0.1.0"},
            },
        )
        self._notify("notifications/initialized")

    def _notify(self, method: str, params: Optional[dict] = None) -> None:
        payload: dict = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            payload["params"] = params
        self._write(payload)

    def _call(self, method: str, params: Optional[dict] = None, timeout: float = 20.0) -> dict:
        with self._lock:
            self._next_id += 1
            request_id = self._next_id

        payload: dict = {"jsonrpc": "2.0", "id": request_id, "method": method}
        if params is not None:
            payload["params"] = params
        self._write(payload)

        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            while self._lines:
                line = self._lines.popleft()
                try:
                    msg = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if msg.get("id") == request_id:
                    if "error" in msg:
                        raise MCPCallError(msg["error"])
                    return msg["result"]
            time.sleep(0.02)
        raise TimeoutError(f"No response for {method!r} within {timeout}s")

    def _write(self, payload: dict) -> None:
        assert self._process and self._process.stdin
        self._process.stdin.write(json.dumps(payload) + "\n")
        self._process.stdin.flush()

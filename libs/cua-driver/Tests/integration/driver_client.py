"""Minimal MCP stdio client used by cua-driver integration tests.

Speaks JSON-RPC 2.0 line-framed over stdin/stdout. Keeps request IDs monotonic
and strips notifications from the response stream. Deliberately tiny — we want
the test failures to point at cua-driver, not at a heavy client dependency.
"""

from __future__ import annotations

import json
import os
import subprocess
import threading
import time
from collections import deque
from typing import Any, Optional


class DriverClient:
    def __init__(self, binary_path: str, subcommand: str = "mcp") -> None:
        self.binary_path = binary_path
        self.subcommand = subcommand
        self.process: Optional[subprocess.Popen] = None
        self._next_id = 0
        self._lines: deque[str] = deque()
        self._reader: Optional[threading.Thread] = None

    def __enter__(self) -> "DriverClient":
        self.process = subprocess.Popen(
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
        return self

    def __exit__(self, *exc: Any) -> None:
        if not self.process:
            return
        for stream in (self.process.stdin, self.process.stdout, self.process.stderr):
            try:
                if stream is not None:
                    stream.close()
            except Exception:
                pass
        try:
            self.process.terminate()
            self.process.wait(timeout=3)
        except Exception:
            self.process.kill()
            self.process.wait(timeout=1)

    def _pump_stdout(self) -> None:
        assert self.process and self.process.stdout
        for line in self.process.stdout:
            line = line.strip()
            if line:
                self._lines.append(line)

    def _handshake(self) -> None:
        self._call(
            "initialize",
            {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "cua-driver-integration", "version": "0.0.1"},
            },
        )
        self._notify("notifications/initialized")

    def _notify(self, method: str, params: Optional[dict] = None) -> None:
        payload = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            payload["params"] = params
        self._write(payload)

    def _call(
        self, method: str, params: Optional[dict] = None, timeout: float = 20.0
    ) -> dict:
        self._next_id += 1
        request_id = self._next_id
        payload = {"jsonrpc": "2.0", "id": request_id, "method": method}
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
        raise TimeoutError(f"no response for {method} within {timeout}s")

    def _write(self, payload: dict) -> None:
        assert self.process and self.process.stdin
        self.process.stdin.write(json.dumps(payload) + "\n")
        self.process.stdin.flush()

    def list_tools(self) -> list[dict]:
        return self._call("tools/list")["tools"]

    def call_tool(self, name: str, arguments: Optional[dict] = None) -> dict:
        return self._call(
            "tools/call",
            {"name": name, "arguments": arguments or {}},
        )


class MCPCallError(RuntimeError):
    def __init__(self, error: dict) -> None:
        super().__init__(f"MCP error {error.get('code')}: {error.get('message')}")
        self.code = error.get("code")
        self.message = error.get("message")


def default_binary_path() -> str:
    return os.environ.get(
        "CUA_DRIVER_BINARY",
        os.path.join(
            os.path.dirname(os.path.abspath(__file__)),
            "..",
            "..",
            ".build",
            "debug",
            "cua-driver",
        ),
    )


def reset_calculator(settle_s: float = 0.3) -> None:
    """Force a known clean state for Calculator.

    Several tests drive Calculator as a "freshly launched, never frontmost"
    target. `pkill -x Calculator` + short settle is the shared boilerplate.
    """
    subprocess.run(["pkill", "-x", "Calculator"], check=False)
    time.sleep(settle_s)


def frontmost_bundle_id(client: "DriverClient") -> Optional[str]:
    """Return the bundle id of the currently-frontmost app, or None.

    Several tests care about "who's on top right now" as a focus-steal probe.
    """
    apps = client.call_tool("list_apps")["structuredContent"]["apps"]
    for app in apps:
        if app.get("active"):
            return app.get("bundle_id")
    return None


def resolve_window_id(
    client: "DriverClient", pid: int, require_on_current_space: bool = True
) -> int:
    """Pick a `window_id` for `pid` via `list_windows`.

    Prefers windows that are on-screen AND on the user's current Space,
    then max z_index. Falls back to any layer-0 window for the pid when
    the preferred filter finds nothing (hidden-launched / minimized apps
    still have layer-0 windows we can snapshot).

    Raises `RuntimeError` when the pid has no layer-0 window at all —
    there's nothing for `get_window_state` to target then.
    """
    result = client.call_tool("list_windows", {"pid": pid})
    windows = result["structuredContent"]["windows"]
    if not windows:
        raise RuntimeError(f"pid {pid} has no windows")

    if require_on_current_space:
        preferred = [
            w for w in windows
            if w.get("is_on_screen") and w.get("on_current_space") is not False
        ]
        if preferred:
            preferred.sort(key=lambda w: w.get("z_index", 0), reverse=True)
            return preferred[0]["window_id"]

    # Fallback: any layer-0 window. Pick the one with the biggest
    # bounds — matches the driver's own max-area fallback for
    # hidden / minimized targets.
    def _area(w: dict) -> float:
        b = w.get("bounds", {})
        return float(b.get("width", 0)) * float(b.get("height", 0))

    windows.sort(key=_area, reverse=True)
    return windows[0]["window_id"]

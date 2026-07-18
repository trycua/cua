"""MCP transport implementations for the experimental Python client."""

from __future__ import annotations

import json
import os
import subprocess
import threading
from collections.abc import Mapping, Sequence
from typing import Any, Protocol, TextIO

from ._generated import MCP_PROTOCOL_VERSION


class Transport(Protocol):
    def request(self, method: str, params: Mapping[str, Any] | None = None) -> Mapping[str, Any]: ...

    def close(self) -> None: ...


class McpResponseError(RuntimeError):
    def __init__(self, code: int | None, message: str):
        self.code = code
        super().__init__(message)


class StdioMcpTransport:
    """Line-delimited JSON-RPC client for `cua-driver mcp`."""

    def __init__(
        self,
        command: Sequence[str] = ("cua-driver", "mcp"),
        *,
        cwd: str | None = None,
        env: Mapping[str, str] | None = None,
    ) -> None:
        if not command:
            raise ValueError("command must not be empty")
        self._command = tuple(command)
        self._cwd = cwd
        self._env = dict(env) if env is not None else None
        self._process: subprocess.Popen[str] | None = None
        self._stdin: TextIO | None = None
        self._stdout: TextIO | None = None
        self._next_id = 1
        self._started = False
        self._lock = threading.RLock()

    def request(self, method: str, params: Mapping[str, Any] | None = None) -> Mapping[str, Any]:
        with self._lock:
            self._ensure_started()
            return self._exchange(method, params)

    def close(self) -> None:
        with self._lock:
            process = self._process
            self._process = None
            self._stdin = None
            self._stdout = None
            self._started = False
            if process is None:
                return
            if process.stdin:
                process.stdin.close()
            try:
                process.wait(timeout=1)
            except subprocess.TimeoutExpired:
                process.terminate()
                try:
                    process.wait(timeout=1)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=1)

    def __enter__(self) -> "StdioMcpTransport":
        with self._lock:
            self._ensure_started()
        return self

    def __exit__(self, *_error: object) -> None:
        self.close()

    def _ensure_started(self) -> None:
        if self._started:
            return
        environment = os.environ.copy()
        if self._env:
            environment.update(self._env)
        process = subprocess.Popen(
            self._command,
            cwd=self._cwd,
            env=environment,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=None,
            text=True,
            encoding="utf-8",
            bufsize=1,
        )
        if process.stdin is None or process.stdout is None:
            process.terminate()
            raise RuntimeError("failed to open cua-driver MCP stdio pipes")
        self._process = process
        self._stdin = process.stdin
        self._stdout = process.stdout
        try:
            self._exchange(
                "initialize",
                {
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "cua-driver-python-client", "version": "0.0.0"},
                },
            )
            self._notify("notifications/initialized")
            self._started = True
        except Exception:
            self.close()
            raise

    def _notify(self, method: str, params: Mapping[str, Any] | None = None) -> None:
        payload: dict[str, Any] = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            payload["params"] = dict(params)
        self._write(payload)

    def _exchange(self, method: str, params: Mapping[str, Any] | None) -> Mapping[str, Any]:
        request_id = self._next_id
        self._next_id += 1
        payload: dict[str, Any] = {"jsonrpc": "2.0", "id": request_id, "method": method}
        if params is not None:
            payload["params"] = dict(params)
        self._write(payload)
        while True:
            response = self._read()
            if response.get("id") != request_id:
                continue
            error = response.get("error")
            if isinstance(error, Mapping):
                code = error.get("code") if isinstance(error.get("code"), int) else None
                message = error.get("message")
                raise McpResponseError(code, message if isinstance(message, str) else str(error))
            result = response.get("result")
            if not isinstance(result, Mapping):
                raise McpResponseError(None, f"MCP response for {method} has no object result")
            return result

    def _write(self, payload: Mapping[str, Any]) -> None:
        if self._stdin is None:
            raise RuntimeError("MCP transport is not running")
        self._stdin.write(json.dumps(payload, separators=(",", ":")) + "\n")
        self._stdin.flush()

    def _read(self) -> Mapping[str, Any]:
        if self._stdout is None:
            raise RuntimeError("MCP transport is not running")
        line = self._stdout.readline()
        if not line:
            code = self._process.poll() if self._process else None
            raise RuntimeError(f"cua-driver MCP process exited unexpectedly (code={code})")
        value = json.loads(line)
        if not isinstance(value, Mapping):
            raise RuntimeError("cua-driver MCP emitted a non-object JSON message")
        return value

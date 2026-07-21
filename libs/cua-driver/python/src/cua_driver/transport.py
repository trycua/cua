"""MCP transport implementations for the cua-driver Python SDK."""

from __future__ import annotations

import asyncio
import json
import os
import queue
import subprocess
import threading
from collections.abc import Mapping, Sequence
from typing import Any, Protocol, TextIO

from ._generated import MCP_PROTOCOL_VERSION


class Transport(Protocol):
    def request(self, method: str, params: Mapping[str, Any] | None = None) -> Mapping[str, Any]: ...

    def close(self) -> None: ...


class AsyncTransport(Protocol):
    async def request(
        self, method: str, params: Mapping[str, Any] | None = None
    ) -> Mapping[str, Any]: ...

    async def close(self) -> None: ...


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
        timeout: float = 30.0,
    ) -> None:
        if not command:
            raise ValueError("command must not be empty")
        if timeout <= 0:
            raise ValueError("timeout must be positive")
        self._command = tuple(command)
        self._cwd = cwd
        self._env = dict(env) if env is not None else None
        self._timeout = timeout
        self._process: subprocess.Popen[str] | None = None
        self._stdin: TextIO | None = None
        self._stdout: TextIO | None = None
        self._next_id = 1
        self._started = False
        self._lock = threading.RLock()
        self._responses: queue.Queue[str | None] = queue.Queue()
        self._reader: threading.Thread | None = None

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
            reader = self._reader
            self._reader = None
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
            if process.stdout:
                process.stdout.close()
            if reader:
                reader.join(timeout=1)

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
        self._responses = queue.Queue()
        self._reader = threading.Thread(
            target=self._pump_stdout,
            args=(process.stdout,),
            name="cua-driver-mcp-reader",
            daemon=True,
        )
        self._reader.start()
        try:
            self._exchange(
                "initialize",
                {
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "cua-driver-python", "version": "0.0.0"},
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
        try:
            line = self._responses.get(timeout=self._timeout)
        except queue.Empty as error:
            raise TimeoutError("cua-driver MCP request timed out") from error
        if line is None:
            code = self._process.poll() if self._process else None
            raise RuntimeError(f"cua-driver MCP process exited unexpectedly (code={code})")
        value = json.loads(line)
        if not isinstance(value, Mapping):
            raise RuntimeError("cua-driver MCP emitted a non-object JSON message")
        return value

    def _pump_stdout(self, stream: TextIO) -> None:
        try:
            for line in stream:
                self._responses.put(line)
        finally:
            self._responses.put(None)


class AsyncStdioMcpTransport:
    """Async line-delimited JSON-RPC client for `cua-driver mcp`."""

    def __init__(
        self,
        command: Sequence[str] = ("cua-driver", "mcp"),
        *,
        cwd: str | None = None,
        env: Mapping[str, str] | None = None,
        timeout: float = 30.0,
    ) -> None:
        if not command:
            raise ValueError("command must not be empty")
        if timeout <= 0:
            raise ValueError("timeout must be positive")
        self._command = tuple(command)
        self._cwd = cwd
        self._env = dict(env) if env is not None else None
        self._timeout = timeout
        self._process: asyncio.subprocess.Process | None = None
        self._next_id = 1
        self._started = False
        self._lock = asyncio.Lock()

    async def request(
        self, method: str, params: Mapping[str, Any] | None = None
    ) -> Mapping[str, Any]:
        async with self._lock:
            await self._ensure_started()
            return await self._exchange(method, params)

    async def close(self) -> None:
        async with self._lock:
            process = self._process
            self._process = None
            self._started = False
            if process is None:
                return
            if process.stdin:
                process.stdin.close()
                await process.stdin.wait_closed()
            try:
                await asyncio.wait_for(process.wait(), 1.0)
            except asyncio.TimeoutError:
                process.terminate()
                try:
                    await asyncio.wait_for(process.wait(), 1.0)
                except asyncio.TimeoutError:
                    process.kill()
                    await process.wait()

    async def __aenter__(self) -> "AsyncStdioMcpTransport":
        async with self._lock:
            await self._ensure_started()
        return self

    async def __aexit__(self, *_error: object) -> None:
        await self.close()

    async def _ensure_started(self) -> None:
        if self._started:
            return
        environment = os.environ.copy()
        if self._env:
            environment.update(self._env)
        self._process = await asyncio.create_subprocess_exec(
            *self._command,
            cwd=self._cwd,
            env=environment,
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
        )
        try:
            await self._exchange(
                "initialize",
                {
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {
                        "name": "cua-driver-python-async",
                        "version": "0.0.0",
                    },
                },
            )
            await self._notify("notifications/initialized")
            self._started = True
        except Exception:
            process = self._process
            self._process = None
            if process and process.returncode is None:
                process.kill()
                await process.wait()
            raise

    async def _notify(
        self, method: str, params: Mapping[str, Any] | None = None
    ) -> None:
        payload: dict[str, Any] = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            payload["params"] = dict(params)
        await self._write(payload)

    async def _exchange(
        self, method: str, params: Mapping[str, Any] | None
    ) -> Mapping[str, Any]:
        request_id = self._next_id
        self._next_id += 1
        payload: dict[str, Any] = {
            "jsonrpc": "2.0",
            "id": request_id,
            "method": method,
        }
        if params is not None:
            payload["params"] = dict(params)
        await self._write(payload)
        while True:
            response = await self._read()
            if response.get("id") != request_id:
                continue
            error = response.get("error")
            if isinstance(error, Mapping):
                code = error.get("code") if isinstance(error.get("code"), int) else None
                message = error.get("message")
                raise McpResponseError(
                    code, message if isinstance(message, str) else str(error)
                )
            result = response.get("result")
            if not isinstance(result, Mapping):
                raise McpResponseError(
                    None, f"MCP response for {method} has no object result"
                )
            return result

    async def _write(self, payload: Mapping[str, Any]) -> None:
        process = self._process
        if process is None or process.stdin is None:
            raise RuntimeError("MCP transport is not running")
        process.stdin.write(
            (json.dumps(payload, separators=(",", ":")) + "\n").encode("utf-8")
        )
        try:
            await asyncio.wait_for(process.stdin.drain(), self._timeout)
        except asyncio.TimeoutError as error:
            raise TimeoutError("cua-driver MCP write timed out") from error

    async def _read(self) -> Mapping[str, Any]:
        process = self._process
        if process is None or process.stdout is None:
            raise RuntimeError("MCP transport is not running")
        try:
            line = await asyncio.wait_for(process.stdout.readline(), self._timeout)
        except asyncio.TimeoutError as error:
            raise TimeoutError("cua-driver MCP request timed out") from error
        if not line:
            code = process.returncode
            raise RuntimeError(f"cua-driver MCP process exited unexpectedly (code={code})")
        value = json.loads(line)
        if not isinstance(value, Mapping):
            raise RuntimeError("cua-driver MCP emitted a non-object JSON message")
        return value

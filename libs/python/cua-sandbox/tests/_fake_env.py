"""An in-memory stand-in for ``cua.SpacesdClient`` used by the EnvTransport tests.

It records every call so the tests can assert exactly which spacesd RPC an
interface action became (the replacement for the old computer-server wire
tests). Methods mirror the generated binding's names and signatures.
"""

from __future__ import annotations

import json
from types import SimpleNamespace
from typing import Any, Optional


class FakeProcess:
    def __init__(self, pid: int) -> None:
        self._pid = pid
        self.pty_input: list[bytes] = []
        self.killed = False
        self.detached = False

    def pid(self) -> int:
        return self._pid

    async def write_pty(self, data: bytes) -> None:
        self.pty_input.append(bytes(data))

    async def kill(self) -> None:
        self.killed = True

    async def detach(self) -> None:
        self.detached = True


class FakeEnv:
    def __init__(self, *, windows: Optional[list[dict]] = None, os_family: str = "linux") -> None:
        self.calls: list[tuple] = []
        self.files: dict[str, bytes] = {}
        self.dirs: set[str] = {"/"}
        self.clipboard: Optional[str] = None
        self.windows = windows if windows is not None else []
        self.os_family = os_family
        self.processes: list[FakeProcess] = []

    def _record(self, *call: Any) -> None:
        self.calls.append(call)

    # computer
    async def click(self, x: float, y: float) -> None:
        self._record("click", x, y)

    async def right_click(self, x: float, y: float) -> None:
        self._record("right_click", x, y)

    async def double_click(self, x: float, y: float) -> None:
        self._record("double_click", x, y)

    async def move_to(self, x: float, y: float) -> None:
        self._record("move_to", x, y)

    async def drag(self, fx: float, fy: float, tx: float, ty: float) -> None:
        self._record("drag", fx, fy, tx, ty)

    async def pointer_json(self, request_json: str) -> str:
        self._record("pointer_json", json.loads(request_json))
        return "{}"

    async def keyboard_json(self, request_json: str) -> str:
        self._record("keyboard_json", json.loads(request_json))
        return "{}"

    async def type_text(self, text: str) -> None:
        self._record("type_text", text)

    async def press(self, key: str) -> None:
        self._record("press", key)

    async def hotkey(self, keys: list[str]) -> None:
        self._record("hotkey", list(keys))

    async def get_clipboard(self) -> Optional[str]:
        return self.clipboard

    async def set_clipboard(self, text: str) -> int:
        self.clipboard = text
        return 1

    async def cursor_position(self) -> Any:
        return SimpleNamespace(x=3.0, y=4.0)

    async def displays(self) -> str:
        return json.dumps(
            [{"id": "primary", "primary": True, "nativeSize": {"width": 1280, "height": 800}}]
        )

    async def screenshot(self, options: Any) -> Any:
        self._record("screenshot", options)
        return SimpleNamespace(
            image=b"\x89PNG\r\n\x1a\n" + b"0" * 16, width=1280, height=800, format=None
        )

    async def capabilities(self) -> Any:
        return SimpleNamespace(os_family=self.os_family)

    async def call_json(self, method: str, request_json: str) -> str:
        self._record("call_json", method, json.loads(request_json))
        if method.endswith("WindowsService/ListWindows"):
            return json.dumps({"windows": self.windows})
        return "{}"

    # process
    async def sh(self, line: str, timeout_ms: Optional[int]) -> Any:
        self._record("sh", line, timeout_ms)
        code = 3 if line.startswith("false") else 0
        return SimpleNamespace(
            exit=SimpleNamespace(
                code=code, signal=None, timed_out=False, error=None, success=code == 0
            ),
            stdout=b"out\n",
            stderr=b"" if code == 0 else b"boom\n",
            pty=b"",
        )

    async def spawn(self, command: Any) -> FakeProcess:
        self._record("spawn", command.program, list(command.args), command.pty)
        process = FakeProcess(100 + len(self.processes))
        self.processes.append(process)
        return process

    async def list_processes(self, include_exited: bool) -> str:
        return json.dumps(
            {
                "processes": [
                    {"pid": p.pid(), "state": "PROCESS_STATE_RUNNING"} for p in self.processes
                ]
            }
        )

    # filesystem
    async def upload(self, path: str, data: bytes, options: Any) -> Any:
        self.files[path] = bytes(data)
        return SimpleNamespace(size=len(data), sha256="", resumes=0)

    async def download(self, path: str) -> bytes:
        if path not in self.files:
            raise FileNotFoundError(path)
        return self.files[path]

    async def stat(self, path: str) -> Any:
        if path in self.files:
            return SimpleNamespace(kind="file", size=len(self.files[path]), path=path, name=path)
        if path in self.dirs:
            return SimpleNamespace(kind="directory", size=0, path=path, name=path)
        raise _not_found(path)

    async def list_dir(self, path: str, depth: int) -> list:
        prefix = path.rstrip("/") + "/"
        return [
            SimpleNamespace(name=p[len(prefix) :], path=p, kind="file", size=len(d))
            for p, d in self.files.items()
            if p.startswith(prefix)
        ]

    async def make_dir(self, path: str) -> Any:
        self.dirs.add(path)
        return SimpleNamespace(kind="directory", path=path)

    async def remove(self, path: str, recursive: bool) -> None:
        self._record("remove", path, recursive)
        self.files.pop(path, None)
        self.dirs.discard(path)


def _not_found(path: str) -> Exception:
    try:
        from cua._native import CuaError

        return CuaError.NotFound(path)
    except Exception:  # pragma: no cover - SDK missing
        return FileNotFoundError(path)

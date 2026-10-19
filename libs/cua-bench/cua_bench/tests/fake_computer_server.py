"""A stand-in for the retired cua computer-server, on loopback, for tests.

It speaks the wire protocol cua-computer 0.5 and cua-bench's compatibility
client use (``POST /cmd`` with ``{"command", "params"}``, one SSE frame
``data: {json}`` back) against an in-memory filesystem. Shell commands are
never executed: a few file commands (``mkdir -p``, ``rm``, ``cat``, ``echo``,
``touch``, ``test -f/-d``) are interpreted, everything else is recorded and
answered with exit code 0 (or ``exit_codes`` overrides).
"""

from __future__ import annotations

import base64
import json
import posixpath
import re
import shlex
from typing import Any, Optional

from aiohttp import web

#: Bounds so a misbehaving client can never make the fake grow without limit.
MAX_COMMANDS = 20_000
MAX_FILE_BYTES = 64 * 1024 * 1024


class FakeComputerServer:
    def __init__(self) -> None:
        self.files: dict[str, bytes] = {}
        self.dirs: set[str] = {"/", "/tmp"}
        self.commands: list[tuple[str, dict]] = []
        self.shell: list[str] = []
        self.exit_codes: dict[str, int] = {}
        self.fail_next: list[str] = []  # command names answered with a transport-ish error
        self._runner: Optional[web.AppRunner] = None
        self.port: Optional[int] = None

    # lifecycle -----------------------------------------------------------------

    async def start(self, host: str = "127.0.0.1") -> "FakeComputerServer":
        app = web.Application()
        app.router.add_post("/cmd", self._handle)
        app.router.add_get("/status", self._status)
        self._runner = web.AppRunner(app)
        await self._runner.setup()
        site = web.TCPSite(self._runner, host, 0)
        await site.start()
        self.port = site._server.sockets[0].getsockname()[1]  # type: ignore[union-attr]
        return self

    async def _status(self, _request: web.Request) -> web.Response:
        return web.json_response({"status": "ok"})

    async def stop(self) -> None:
        if self._runner is not None:
            await self._runner.cleanup()
            self._runner = None

    @property
    def url(self) -> str:
        return f"http://127.0.0.1:{self.port}"

    # protocol ------------------------------------------------------------------

    async def _handle(self, request: web.Request) -> web.Response:
        body = await request.json()
        command, params = body.get("command", ""), dict(body.get("params") or {})
        if len(self.commands) < MAX_COMMANDS:
            self.commands.append((command, params))
        if self.fail_next and self.fail_next[0] == command:
            self.fail_next.pop(0)
            reply: dict[str, Any] = {"success": False, "error": "Connection reset by peer"}
        else:
            try:
                reply = {"success": True, **(self._dispatch(command, params) or {})}
            except FileNotFoundError as error:
                reply = {"success": False, "error": f"No such file or directory: {error}"}
            except KeyError as error:
                reply = {"success": False, "error": f"Unknown command: {error}"}
        return web.Response(text="data: " + json.dumps(reply) + "\n\n")

    def _dispatch(self, command: str, p: dict) -> dict:
        path = p.get("path")
        if command == "version":
            return {"protocol": 1, "package": "fake-computer-server"}
        if command == "get_screen_size":
            return {"size": {"width": 1280, "height": 800}}
        if command == "screenshot":
            return {"image_data": base64.b64encode(_png()).decode()}
        if command == "file_exists":
            return {"exists": path in self.files}
        if command == "directory_exists":
            return {"exists": path in self.dirs}
        if command == "create_dir":
            self._mkdirs(path)
            return {}
        if command == "delete_dir":
            self.dirs.discard(path)
            for key in [k for k in self.files if k.startswith(path.rstrip("/") + "/")]:
                del self.files[key]
            return {}
        if command == "delete_file":
            if path not in self.files:
                raise FileNotFoundError(path)
            del self.files[path]
            return {}
        if command == "list_dir":
            base = path.rstrip("/") or "/"
            children = {
                posixpath.basename(k)
                for k in (*self.files, *self.dirs)
                if k != base and posixpath.dirname(k) == base
            }
            return {"files": sorted(children)}
        if command == "get_file_size":
            if path not in self.files:
                raise FileNotFoundError(path)
            return {"size": len(self.files[path])}
        if command == "read_bytes":
            if path not in self.files:
                raise FileNotFoundError(path)
            data = self.files[path]
            offset, length = int(p.get("offset") or 0), p.get("length")
            data = data[offset : None if length is None else offset + int(length)]
            return {"content_b64": base64.b64encode(data).decode()}
        if command == "write_bytes":
            data = base64.b64decode(p.get("content_b64", ""))
            old = self.files.get(path, b"") if p.get("append") else b""
            if len(old) + len(data) > MAX_FILE_BYTES:
                raise KeyError("file too large for the fake")
            self._mkdirs(posixpath.dirname(path))
            self.files[path] = old + data
            return {}
        if command == "run_command":
            return self._run(p.get("command", ""))
        if command in (
            "left_click",
            "right_click",
            "double_click",
            "move_cursor",
            "drag_to",
            "scroll_up",
            "scroll_down",
            "type_text",
            "press_key",
            "hotkey",
            "launch",
            "open",
            "mouse_down",
            "mouse_up",
        ):
            return {"pid": 4242} if command == "launch" else {}
        raise KeyError(command)

    def _mkdirs(self, path: Optional[str]) -> None:
        while path and path not in self.dirs:
            self.dirs.add(path)
            path = posixpath.dirname(path)

    def _run(self, line: str) -> dict:
        if len(self.shell) < MAX_COMMANDS:
            self.shell.append(line)
        out, code = "", self.exit_codes.get(line, 0)
        try:
            argv = shlex.split(line)
        except ValueError:
            argv = line.split()
        head = argv[0] if argv else ""
        args = [a for a in argv[1:] if not a.startswith("-")]
        detached = self._detached(line)
        if detached is not None:
            return detached
        if head == "mkdir":
            for path in args:
                self._mkdirs(path)
        elif head == "rm":
            for path in args:
                self.files.pop(path, None)
        elif head == "touch":
            for path in args:
                self.files.setdefault(path, b"")
        elif head == "cat":
            out = "".join(self.files.get(a, b"").decode(errors="replace") for a in args)
        elif head == "echo":
            out = " ".join(argv[1:]) + "\n"
        elif head == "test" and len(argv) == 3:
            present = argv[2] in (self.files if argv[1] == "-f" else self.dirs)
            code = 0 if present else 1
        return {"stdout": out, "stderr": "", "return_code": code}

    # Harnesses run long commands detached: write a wrapper script, launch it
    # in the background, poll a done-marker, read out/err/rc files (ALE's
    # driver does this). Emulate the launch by "finishing" the job at once.
    _SH_WRAP = re.compile(r"nohup bash '([^']+)'")
    _BAT_WRAP = re.compile(r'start "" /b cmd /c "([^"]+)"')

    def _detached(self, line: str) -> Optional[dict]:
        match = self._SH_WRAP.search(line) or self._BAT_WRAP.search(line)
        if match is not None:
            script = self.files.get(match.group(1), b"").decode(errors="replace")
            sh = re.search(
                r"> '([^']+)' 2> '([^']+)'\s+echo \$\? > '([^']+)'\s+touch '([^']+)'", script
            )
            bat = re.search(
                r'> "([^"]+)" 2> "([^"]+)".*?> "([^"]+)" echo !ALE_RC!.*?echo done> "([^"]+)"',
                script,
                re.S,
            )
            found = sh or bat
            if found:
                out_f, err_f, rc_f, done_f = found.groups()
                self.files[out_f], self.files[err_f] = b"", b""
                self.files[rc_f], self.files[done_f] = b"0\n", b""
            return {"stdout": "", "stderr": "", "return_code": 0}
        probe = re.search(r"\[ -f '([^']+)' \] && echo __DONE__", line) or re.search(
            r'if exist "([^"]+)" \(echo __DONE__\)', line
        )
        if probe is not None:
            done = probe.group(1) in self.files
            return {
                "stdout": "__DONE__\n" if done else "__WAIT__\n",
                "stderr": "",
                "return_code": 0,
            }
        return None


def _png() -> bytes:
    from io import BytesIO

    from PIL import Image

    buf = BytesIO()
    Image.new("RGB", (4, 3), (0, 0, 0)).save(buf, format="PNG")
    return buf.getvalue()

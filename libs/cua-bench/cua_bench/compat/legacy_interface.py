"""The cua-computer 0.5 interface surface, kept for code written against it.

cua-bench 0.2.x handed tasks and harnesses a cua-computer ``Computer`` and
its ``interface`` (``session.computer`` / ``session.interface``). cua-computer
and computer-server were retired, but Agents' Last Exam and other users still
call that surface (``create_dir``, ``run_command -> CommandResult``,
``write_text(append=)``, a patchable ``_send_command`` ...), and their VMs
still run computer-server. Two implementations share one method set:

* :class:`ComputerServerInterface`: a small client for a computer-server
  (``POST /cmd``, SSE-framed replies), the transport ``RemoteDesktopSession``
  uses for ``api_url`` sessions that are not cua-spacesd.
* :class:`SandboxInterface`: the same methods over a ``cua_sandbox.Sandbox``
  (cua-spacesd), so ``session.interface.create_dir(...)`` keeps working on the
  canonical images. Unknown attributes fall through to the sandbox
  (``session.interface.files``, ``.shell`` ...).

Method semantics follow cua-computer 0.5.17, e.g. ``file_exists`` is true for
files only and ``run_command`` returns the real exit code in
``CommandResult.returncode``.
"""

from __future__ import annotations

import asyncio
import base64
import json
import logging
import shlex
import time
from dataclasses import dataclass
from typing import Any, Dict, List, Optional, Tuple

logger = logging.getLogger(__name__)

#: Transfers above this go in chunks (computer-server's limit per message).
CHUNK_SIZE = 1024 * 1024
LARGE_FILE = 5 * 1024 * 1024
#: Retries when a proxy cuts the SSE reply off (bounded).
EMPTY_REPLY_RETRIES = 3


@dataclass
class CommandResult:
    """``computer.interface.models.CommandResult`` (note: ``returncode``)."""

    stdout: str
    stderr: str
    returncode: int


def _b64(data: bytes) -> str:
    return base64.b64encode(data).decode("ascii")


def _unb64(text: str) -> bytes:
    return base64.b64decode(text or "")


class LegacyInterface:
    """cua-computer's ``BaseComputerInterface`` methods over ``_send_command``.

    Subclasses implement :meth:`_send_command` (the one method harnesses wrap
    for retries) and may override single methods with a direct path.
    """

    #: Marks interfaces cua-bench implements (their run_command takes timeout=).
    _cua_bench_interface = True

    def __init__(self, ip_address: str = "localhost", api_port: Optional[int] = None) -> None:
        self.ip_address = ip_address
        self._api_port = api_port
        self.delay = 0.0

    async def _send_command(self, command: str, params: Optional[Dict] = None) -> Dict[str, Any]:
        raise NotImplementedError

    async def _ok(self, command: str, params: Optional[Dict] = None, what: str = "") -> Dict:
        result = await self._send_command(command, params or {})
        if not result.get("success", False):
            raise RuntimeError(result.get("error") or f"Failed to {what or command}")
        return result

    async def _delay(self, delay: Optional[float]) -> None:
        wait = self.delay if delay is None else delay
        if wait and wait > 0:
            await asyncio.sleep(wait)

    # lifecycle ---------------------------------------------------------------

    async def wait_for_ready(self, timeout: int = 60, interval: float = 1.0) -> None:
        """Poll ``get_screen_size`` until the server answers (bounded)."""
        deadline = time.monotonic() + timeout
        last: Any = None
        for _ in range(max(1, int(timeout / max(interval, 0.01)) + 1)):
            try:
                result = await self._send_command("get_screen_size", {})
                if result.get("success", False):
                    return
                last = result.get("error")
            except Exception as error:  # noqa: BLE001
                last = error
            if time.monotonic() >= deadline:
                break
            await asyncio.sleep(interval)
        raise TimeoutError(
            f"Could not connect to {self.ip_address} after {timeout} seconds: {last}"
        )

    def close(self) -> None:
        """Keep the server connection for other clients (cua-computer semantics)."""

    def force_close(self) -> None:
        """Drop any open connection."""

    # mouse -------------------------------------------------------------------

    async def mouse_down(self, x=None, y=None, button: str = "left", delay=None) -> None:
        await self._send_command("mouse_down", {"x": x, "y": y, "button": button})
        await self._delay(delay)

    async def mouse_up(self, x=None, y=None, button: str = "left", delay=None) -> None:
        await self._send_command("mouse_up", {"x": x, "y": y, "button": button})
        await self._delay(delay)

    async def left_click(self, x=None, y=None, delay=None) -> None:
        await self._send_command("left_click", {"x": x, "y": y})
        await self._delay(delay)

    async def right_click(self, x=None, y=None, delay=None) -> None:
        await self._send_command("right_click", {"x": x, "y": y})
        await self._delay(delay)

    async def double_click(self, x=None, y=None, delay=None) -> None:
        await self._send_command("double_click", {"x": x, "y": y})
        await self._delay(delay)

    async def move_cursor(self, x: int, y: int, delay=None) -> None:
        await self._send_command("move_cursor", {"x": x, "y": y})
        await self._delay(delay)

    async def drag_to(
        self, x: int, y: int, button: str = "left", duration: float = 0.5, delay=None
    ):
        await self._send_command(
            "drag_to", {"x": x, "y": y, "button": button, "duration": duration}
        )
        await self._delay(delay)

    async def drag(
        self, path: List[Tuple[int, int]], button: str = "left", duration=0.5, delay=None
    ):
        await self._send_command("drag", {"path": path, "button": button, "duration": duration})
        await self._delay(delay)

    async def scroll(self, x: int, y: int, delay=None) -> None:
        await self._send_command("scroll", {"x": x, "y": y})
        await self._delay(delay)

    async def scroll_down(self, clicks: int = 1, delay=None) -> None:
        await self._send_command("scroll_down", {"clicks": clicks})
        await self._delay(delay)

    async def scroll_up(self, clicks: int = 1, delay=None) -> None:
        await self._send_command("scroll_up", {"clicks": clicks})
        await self._delay(delay)

    async def get_cursor_position(self) -> Dict[str, int]:
        return (await self._ok("get_cursor_position"))["position"]

    # keyboard ----------------------------------------------------------------

    @staticmethod
    def _key(key: Any) -> str:
        value = getattr(key, "value", key)
        if not isinstance(value, str):
            raise ValueError(f"Invalid key type: {type(key)}. Must be Key enum or string.")
        return {"command": "cmd"}.get(value, value)

    async def key_down(self, key, delay=None) -> None:
        await self._send_command("key_down", {"key": self._key(key)})
        await self._delay(delay)

    async def key_up(self, key, delay=None) -> None:
        await self._send_command("key_up", {"key": self._key(key)})
        await self._delay(delay)

    async def type_text(self, text: str, delay=None) -> None:
        await self._send_command("type_text", {"text": text})
        await self._delay(delay)

    async def press(self, key, delay=None) -> None:
        await self._send_command("press_key", {"key": self._key(key)})
        await self._delay(delay)

    async def press_key(self, key, delay=None) -> None:
        await self.press(key, delay)

    async def hotkey(self, *keys, delay=None) -> None:
        await self._send_command("hotkey", {"keys": [self._key(k) for k in keys]})
        await self._delay(delay)

    # screen ------------------------------------------------------------------

    async def screenshot(self, *args: Any, **kwargs: Any) -> bytes:
        result = await self._send_command("screenshot", {})
        if not result.get("image_data"):
            raise RuntimeError("Failed to take screenshot, no image data received from server")
        return _unb64(result["image_data"])

    async def get_screen_size(self) -> Dict[str, int]:
        result = await self._send_command("get_screen_size", {})
        if result.get("success") and result.get("size"):
            return result["size"]
        raise RuntimeError("Failed to get screen size")

    async def copy_to_clipboard(self) -> str:
        return (await self._ok("copy_to_clipboard")).get("content", "")

    async def set_clipboard(self, text: str) -> None:
        await self._send_command("set_clipboard", {"text": text})

    async def get_accessibility_tree(self) -> Dict[str, Any]:
        return await self._ok("get_accessibility_tree")

    # files -------------------------------------------------------------------

    async def file_exists(self, path: str) -> bool:
        return bool((await self._send_command("file_exists", {"path": path})).get("exists", False))

    async def directory_exists(self, path: str) -> bool:
        result = await self._send_command("directory_exists", {"path": path})
        return bool(result.get("exists", False))

    async def list_dir(self, path: str) -> list[str]:
        return list((await self._ok("list_dir", {"path": path}, "list directory")).get("files", []))

    async def create_dir(self, path: str) -> None:
        await self._ok("create_dir", {"path": path}, "create directory")

    async def delete_dir(self, path: str) -> None:
        await self._ok("delete_dir", {"path": path}, "delete directory")

    async def delete_file(self, path: str) -> None:
        await self._ok("delete_file", {"path": path}, "delete file")

    async def get_file_size(self, path: str) -> int:
        return int(
            (await self._ok("get_file_size", {"path": path}, "get file size")).get("size", 0)
        )

    async def read_bytes(self, path: str, offset: int = 0, length: Optional[int] = None) -> bytes:
        if length is None:
            size = await self.get_file_size(path)
            if size > LARGE_FILE:
                chunks, cursor, remaining = [], offset, size - offset
                while remaining > 0:
                    step = min(CHUNK_SIZE, remaining)
                    part = await self._ok(
                        "read_bytes", {"path": path, "offset": cursor, "length": step}, "read file"
                    )
                    chunks.append(_unb64(part.get("content_b64", "")))
                    cursor += step
                    remaining -= step
                return b"".join(chunks)
        params = {"path": path, "offset": offset, "length": length}
        return _unb64((await self._ok("read_bytes", params, "read file")).get("content_b64", ""))

    async def write_bytes(self, path: str, content: bytes, append: bool = False) -> None:
        if len(content) <= LARGE_FILE:
            params = {"path": path, "content_b64": _b64(content), "append": append}
            await self._ok("write_bytes", params, "write file")
            return
        for start in range(0, len(content), CHUNK_SIZE):
            params = {
                "path": path,
                "content_b64": _b64(content[start : start + CHUNK_SIZE]),
                "append": append if start == 0 else True,
            }
            await self._ok("write_bytes", params, "write file chunk")

    async def read_text(self, path: str, encoding: str = "utf-8") -> str:
        return (await self.read_bytes(path)).decode(encoding)

    async def write_text(
        self, path: str, content: str, encoding: str = "utf-8", append: bool = False
    ) -> None:
        await self.write_bytes(path, content.encode(encoding), append)

    # desktop / apps ----------------------------------------------------------

    async def open(self, target: str) -> None:
        await self._ok("open", {"target": target}, "open target")

    async def launch(self, app: str, args: Optional[list[str]] = None) -> Optional[int]:
        payload: Dict[str, Any] = {"app": app}
        if args is not None:
            payload["args"] = args
        return (await self._ok("launch", payload, "launch application")).get("pid")

    async def get_desktop_environment(self) -> str:
        return (await self._ok("get_desktop_environment")).get("environment", "unknown")

    # commands ----------------------------------------------------------------

    async def run_command(self, command: str, timeout: Optional[float] = None) -> CommandResult:
        """Run ``command``; ``timeout`` (seconds, optional) bounds the wait."""
        if timeout is not None:
            return await asyncio.wait_for(self.run_command(command), timeout)
        result = await self._ok("run_command", {"command": command}, "run command")
        return CommandResult(
            stdout=result.get("stdout", "") or "",
            stderr=result.get("stderr", "") or "",
            returncode=int(result.get("return_code", 0) or 0),
        )


class ComputerServerInterface(LegacyInterface):
    """A client for a legacy computer-server (``POST <base>/cmd``).

    Replies are one SSE frame, ``data: {json}``. Transport problems come back
    as ``{"success": False, "error": "Request failed", ...}`` like cua-computer,
    so existing retry wrappers keep matching them. ``request_timeout`` bounds
    one request (``None``: no limit, the cua-computer behaviour for long
    commands).
    """

    def __init__(
        self,
        ip_address: str,
        api_port: Optional[int] = None,
        *,
        api_key: Optional[str] = None,
        vm_name: Optional[str] = None,
        api_base_url: Optional[str] = None,
        api_headers: Optional[Dict[str, str]] = None,
        request_timeout: Optional[float] = None,
        os_type: str = "linux",
    ) -> None:
        super().__init__(ip_address, api_port)
        self.api_key = api_key
        self.vm_name = vm_name
        self.os_type = os_type
        self._api_base_url = api_base_url.rstrip("/") if api_base_url else None
        self._api_headers = dict(api_headers or {})
        self._request_timeout = request_timeout
        self._session: Any = None

    @property
    def rest_uri(self) -> str:
        if self._api_base_url:
            return f"{self._api_base_url}/cmd"
        scheme = "https" if self.api_key else "http"
        port = self._api_port or (8443 if self.api_key else 8000)
        return f"{scheme}://{self.ip_address}:{port}/cmd"

    def _headers(self) -> Dict[str, str]:
        headers = {"Content-Type": "application/json"}
        if self.api_key:
            headers["X-API-Key"] = self.api_key
        if self.vm_name:
            headers["X-Container-Name"] = self.vm_name
        headers.update(self._api_headers)
        return headers

    async def _post(self, payload: Dict[str, Any], timeout: Optional[float]) -> tuple[int, str]:
        import aiohttp

        client_timeout = aiohttp.ClientTimeout(total=timeout, sock_connect=30)
        async with aiohttp.ClientSession(timeout=client_timeout) as session:
            async with session.post(self.rest_uri, json=payload, headers=self._headers()) as reply:
                return reply.status, (await reply.text()).strip()

    async def _send_command(self, command: str, params: Optional[Dict] = None) -> Dict[str, Any]:
        timeout = self._request_timeout
        payload = {"command": command, "params": dict(params or {})}
        for attempt in range(EMPTY_REPLY_RETRIES):
            try:
                status, text = await self._post(payload, timeout)
            except Exception as error:  # noqa: BLE001 - reported like cua-computer
                logger.debug("computer-server %s failed: %r", command, error)
                return {"success": False, "error": "Request failed", "message": str(error)}
            if not text and attempt < EMPTY_REPLY_RETRIES - 1:
                await asyncio.sleep(2**attempt)
                continue
            if text.startswith("data: "):
                try:
                    return json.loads(text[6:])
                except json.JSONDecodeError:
                    pass
            return {
                "success": False,
                "error": "Server returned malformed response",
                "message": text[:500],
                "status": status,
            }
        return {"success": False, "error": "Server returned malformed response", "message": ""}


class SandboxInterface(LegacyInterface):
    """The cua-computer interface over a connected ``cua_sandbox.Sandbox``.

    Everything the legacy surface has is answered by the sandbox's public
    interfaces (``files``, ``shell``, ``mouse``, ``keyboard``, ``screen``);
    other attributes are the sandbox's own.
    """

    def __init__(self, sandbox: Any, os_type: str = "linux") -> None:
        super().__init__("sandbox")
        self._sb = sandbox
        self.os_type = os_type
        self._cursor: Tuple[int, int] = (0, 0)

    def __getattr__(self, name: str) -> Any:
        if name.startswith("__") or name in ("_sb", "_cursor"):
            raise AttributeError(name)
        return getattr(self._sb, name)

    @property
    def sandbox(self) -> Any:
        return self._sb

    async def _send_command(self, command: str, params: Optional[Dict] = None) -> Dict[str, Any]:
        """computer-server command names, answered by the sandbox."""
        p = dict(params or {})
        handler = _SANDBOX_COMMANDS.get(command)
        if handler is None:
            return {"success": False, "error": f"unsupported command: {command}"}
        try:
            result = await handler(self, p)
        except Exception as error:  # noqa: BLE001 - computer-server reported errors, not raised
            return {"success": False, "error": f"{type(error).__name__}: {error}"}
        return {"success": True, **(result or {})}

    # direct paths (no base64 round trip) -------------------------------------

    async def file_exists(self, path: str) -> bool:
        files = self._sb.files
        try:
            return bool(await files.exists(path)) and not bool(await files.is_dir(path))
        except Exception:  # noqa: BLE001
            return False

    async def directory_exists(self, path: str) -> bool:
        files = self._sb.files
        try:
            return bool(await files.exists(path)) and bool(await files.is_dir(path))
        except Exception:  # noqa: BLE001
            return False

    async def read_bytes(self, path: str, offset: int = 0, length: Optional[int] = None) -> bytes:
        try:
            data = await self._sb.files.read_bytes(path)
        except Exception as error:  # noqa: BLE001
            raise RuntimeError(f"Failed to read file: {error}") from error
        end = None if length is None else offset + length
        return data[offset:end] if (offset or end is not None) else data

    async def write_bytes(self, path: str, content: bytes, append: bool = False) -> None:
        try:
            if append and await self.file_exists(path):
                content = await self._sb.files.read_bytes(path) + content
            await self._sb.files.write_bytes(path, content)
        except Exception as error:  # noqa: BLE001
            raise RuntimeError(f"Failed to write file: {error}") from error

    async def read_text(self, path: str, encoding: str = "utf-8") -> str:
        if encoding.lower().replace("-", "") != "utf8":
            return (await self.read_bytes(path)).decode(encoding)
        try:
            return await self._sb.files.read_text(path)
        except Exception as error:  # noqa: BLE001
            raise RuntimeError(f"Failed to read file: {error}") from error

    async def write_text(
        self, path: str, content: str, encoding: str = "utf-8", append: bool = False
    ) -> None:
        if append or encoding.lower().replace("-", "") != "utf8":
            await self.write_bytes(path, content.encode(encoding), append)
            return
        try:
            await self._sb.files.write_text(path, content)
        except Exception as error:  # noqa: BLE001
            raise RuntimeError(f"Failed to write file: {error}") from error

    async def screenshot(self, *args: Any, **kwargs: Any) -> bytes:
        return await self._sb.screenshot()

    async def get_screen_size(self) -> Dict[str, int]:
        width, height = await self._sb.screen.size()
        return {"width": int(width), "height": int(height)}

    async def run_command(self, command: str, timeout: Optional[float] = None) -> CommandResult:
        result = await self._sb.shell.run(command, timeout=timeout)
        return CommandResult(
            stdout=getattr(result, "stdout", "") or "",
            stderr=getattr(result, "stderr", "") or "",
            returncode=int(getattr(result, "returncode", 0) or 0),
        )

    async def get_accessibility_tree(self) -> Dict[str, Any]:
        return {}


def _xy(p: Dict) -> Tuple[Any, Any]:
    return p.get("x"), p.get("y")


async def _cmd_run(iface: SandboxInterface, p: Dict) -> Dict:
    r = await iface.run_command(p["command"], timeout=p.get("timeout"))
    return {"stdout": r.stdout, "stderr": r.stderr, "return_code": r.returncode}


async def _cmd_read(iface: SandboxInterface, p: Dict) -> Dict:
    data = await iface.read_bytes(p["path"], int(p.get("offset") or 0), p.get("length"))
    return {"content_b64": _b64(data)}


async def _cmd_write(iface: SandboxInterface, p: Dict) -> Dict:
    await iface.write_bytes(p["path"], _unb64(p.get("content_b64", "")), bool(p.get("append")))
    return {}


async def _cmd_list(iface: SandboxInterface, p: Dict) -> Dict:
    return {"files": [e.name for e in await iface._sb.files.list(p["path"])]}


async def _cmd_size(iface: SandboxInterface, p: Dict) -> Dict:
    return {"size": int(await iface._sb.files.size(p["path"]))}


async def _cmd_screenshot(iface: SandboxInterface, p: Dict) -> Dict:
    return {"image_data": _b64(await iface.screenshot())}


async def _cmd_screen_size(iface: SandboxInterface, p: Dict) -> Dict:
    return {"size": await iface.get_screen_size()}


async def _cmd_launch(iface: SandboxInterface, p: Dict) -> Dict:
    line = " ".join(shlex.quote(a) for a in [p["app"], *(p.get("args") or [])])
    r = await iface._sb.shell.run(line, background=True)
    pid = (r.stdout or "").strip()
    return {"pid": int(pid) if pid.isdigit() else None}


async def _cmd_open(iface: SandboxInterface, p: Dict) -> Dict:
    opener = {"windows": "start", "macos": "open"}.get(iface.os_type, "xdg-open")
    await iface._sb.shell.run(f"{opener} {shlex.quote(p['target'])}", background=True)
    return {}


async def _scroll(iface: SandboxInterface, dy: int) -> Dict:
    width, height = await iface._sb.screen.size()
    await iface._sb.mouse.scroll(int(width) // 2, int(height) // 2, scroll_x=0, scroll_y=dy)
    return {}


def _mouse(method: str):
    async def run(iface: SandboxInterface, p: Dict) -> Dict:
        await getattr(iface._sb.mouse, method)(*_xy(p))
        return {}

    return run


async def _cmd_move(iface: SandboxInterface, p: Dict) -> Dict:
    await iface._sb.mouse.move(p["x"], p["y"])
    iface._cursor = (p["x"], p["y"])
    return {}


async def _cmd_drag_to(iface: SandboxInterface, p: Dict) -> Dict:
    # computer-server drags from the current cursor: the last move_cursor.
    x0, y0 = iface._cursor
    await iface._sb.mouse.drag(x0, y0, p["x"], p["y"])
    iface._cursor = (p["x"], p["y"])
    return {}


async def _cmd_keys(iface: SandboxInterface, p: Dict) -> Dict:
    await iface._sb.keyboard.keypress(list(p.get("keys") or []))
    return {}


async def _cmd_key(iface: SandboxInterface, p: Dict) -> Dict:
    await iface._sb.keyboard.keypress(p["key"])
    return {}


async def _cmd_type(iface: SandboxInterface, p: Dict) -> Dict:
    await iface._sb.keyboard.type(p["text"])
    return {}


async def _exists(iface: SandboxInterface, p: Dict) -> Dict:
    return {"exists": await iface.file_exists(p["path"])}


async def _dir_exists(iface: SandboxInterface, p: Dict) -> Dict:
    return {"exists": await iface.directory_exists(p["path"])}


async def _mkdir(iface: SandboxInterface, p: Dict) -> Dict:
    await iface._sb.files.make_dir(p["path"])
    return {}


async def _rmdir(iface: SandboxInterface, p: Dict) -> Dict:
    await iface._sb.files.remove_dir(p["path"])
    return {}


async def _rm(iface: SandboxInterface, p: Dict) -> Dict:
    await iface._sb.files.remove(p["path"])
    return {}


async def _version(iface: SandboxInterface, p: Dict) -> Dict:
    return {"protocol": "cua-spacesd", "package": "cua-bench-compat"}


_SANDBOX_COMMANDS = {
    "run_command": _cmd_run,
    "read_bytes": _cmd_read,
    "write_bytes": _cmd_write,
    "file_exists": _exists,
    "directory_exists": _dir_exists,
    "create_dir": _mkdir,
    "delete_dir": _rmdir,
    "delete_file": _rm,
    "list_dir": _cmd_list,
    "get_file_size": _cmd_size,
    "screenshot": _cmd_screenshot,
    "get_screen_size": _cmd_screen_size,
    "launch": _cmd_launch,
    "open": _cmd_open,
    "left_click": _mouse("click"),
    "right_click": _mouse("right_click"),
    "double_click": _mouse("double_click"),
    "move_cursor": _cmd_move,
    "drag_to": _cmd_drag_to,
    "scroll_down": lambda iface, p: _scroll(iface, int(p.get("clicks", 1))),
    "scroll_up": lambda iface, p: _scroll(iface, -int(p.get("clicks", 1))),
    "type_text": _cmd_type,
    "press_key": _cmd_key,
    "hotkey": _cmd_keys,
    "version": _version,
}


class InterfaceFactory:
    """``computer.interface.factory.InterfaceFactory``."""

    @staticmethod
    def create_interface_for_os(
        os: str,
        ip_address: str,
        api_port: Optional[int] = None,
        api_key: Optional[str] = None,
        vm_name: Optional[str] = None,
        api_base_url: Optional[str] = None,
        api_headers: Optional[Dict[str, str]] = None,
    ) -> ComputerServerInterface:
        if os not in ("macos", "linux", "windows", "android"):
            raise ValueError(f"Unsupported OS type: {os}")
        return ComputerServerInterface(
            ip_address,
            api_port,
            api_key=api_key,
            vm_name=vm_name,
            api_base_url=api_base_url,
            api_headers=api_headers,
            os_type=os,
        )


class Computer:
    """``computer.Computer`` for an existing computer-server (host mode only).

    Provisioning VMs (the old providers) is gone: use ``cb run`` or
    ``cua_sandbox.Sandbox`` for that.
    """

    def __init__(
        self,
        display: Any = None,
        memory: Any = None,
        cpu: Any = None,
        os_type: str = "macos",
        name: str = "",
        image: Any = None,
        shared_directories: Any = None,
        use_host_computer_server: bool = False,
        api_host: Optional[str] = None,
        api_port: Optional[int] = None,
        noVNC_port: Optional[int] = None,
        api_key: Optional[str] = None,
        api_base_url: Optional[str] = None,
        api_headers: Optional[Dict[str, str]] = None,
        **_: Any,
    ) -> None:
        if not use_host_computer_server:
            raise RuntimeError(
                "cua-computer VM providers were retired; this compatibility Computer only "
                "attaches to a running computer-server (use_host_computer_server=True). "
                "Start sandboxes with `cb run` or cua_sandbox.Sandbox instead."
            )
        self.os_type = os_type
        self.use_host_computer_server = True
        self.api_host = api_host or "localhost"
        self.api_port = api_port
        self.noVNC_port = noVNC_port
        self.api_key = api_key
        self._api_base_url = api_base_url
        self._api_headers = api_headers
        self._interface: Any = None
        self._original_interface: Any = None
        self._initialized = False

    @property
    def interface(self) -> Any:
        if self._interface is None:
            self._interface = InterfaceFactory.create_interface_for_os(
                os=self.os_type,
                ip_address=self.api_host,
                api_port=self.api_port,
                api_key=self.api_key,
                api_base_url=self._api_base_url,
                api_headers=self._api_headers,
            )
            self._original_interface = self._interface
        return self._interface

    async def run(self, timeout: int = 60) -> None:
        if not self._initialized:
            await self.interface.wait_for_ready(timeout=timeout)
            self._initialized = True

    async def stop(self) -> None:
        interface = self._interface
        if interface is not None:
            interface.close()

    async def disconnect(self) -> None:
        await self.stop()

"""e2b-style PTY client for a running Computer instance.

Usage::

    async with Computer(provider_type="docker", name="my-container") as c:
        handle = await c.pty.create(
            command="bash",
            cols=80,
            rows=24,
            on_data=lambda d: print(d.decode(errors="replace"), end="", flush=True),
        )
        await handle.send_stdin(b"echo hello\\n")
        await handle.send_stdin(b"exit\\n")
        code = await handle.wait()
        print(f"exited with {code}")
"""

from __future__ import annotations

import asyncio
import base64
import json
import logging
from dataclasses import dataclass, field
from typing import Callable, Optional

logger = logging.getLogger(__name__)


@dataclass
class PtyHandle:
    """Lightweight handle to a live PTY session on a remote computer-server."""

    pid: int
    cols: int
    rows: int
    _iface: "PtyInterface" = field(repr=False)

    async def send_stdin(self, data: bytes) -> None:
        """Write *data* to the PTY's stdin."""
        await self._iface.send_stdin(self.pid, data)

    async def resize(self, cols: int, rows: int) -> None:
        """Resize the terminal window."""
        await self._iface.resize(self.pid, cols, rows)

    async def kill(self) -> bool:
        """Kill the PTY session process."""
        return await self._iface.kill(self.pid)

    async def disconnect(self) -> None:
        """Close the WebSocket connection without killing the PTY.

        The session keeps running on the server; use :meth:`connect` to
        re-attach later.
        """
        await self._iface._disconnect(self.pid)

    async def wait(self) -> int:
        """Block until the PTY session exits and return its exit code.

        Raises:
            LookupError: If the session for this handle's pid is not tracked
                by the interface (e.g. the handle was never connected, or the
                interface was recreated after a reconnect).
        """
        return await self._iface._wait(self.pid)


class PtyInterface:
    """Async HTTP+WebSocket client for the ``/pty`` endpoints on computer-server.

    Args:
        base_url: HTTP base URL of the computer-server, e.g.
            ``"http://192.168.64.10:8000"``.
        api_key: Optional API key for cloud providers (passed as
            ``X-API-Key`` header).
        vm_name: Optional VM / container name (passed as
            ``X-Container-Name`` header).
    """

    def __init__(
        self,
        base_url: str,
        api_key: Optional[str] = None,
        vm_name: Optional[str] = None,
    ) -> None:
        self._base_url = base_url.rstrip("/")
        self._ws_base = self._base_url.replace("https://", "wss://").replace("http://", "ws://")
        self._api_key = api_key
        self._vm_name = vm_name

        # pid → (asyncio.Event, exit_code_cell, ws_task)
        self._sessions: dict[int, dict] = {}

    # ------------------------------------------------------------------
    # Auth helpers
    # ------------------------------------------------------------------

    def _auth_headers(self) -> dict:
        headers = {}
        if self._api_key:
            headers["X-API-Key"] = self._api_key
        if self._vm_name:
            headers["X-Container-Name"] = self._vm_name
        return headers

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    async def create(
        self,
        command: Optional[str] = None,
        cols: int = 80,
        rows: int = 24,
        on_data: Optional[Callable[[bytes], None]] = None,
        cwd: Optional[str] = None,
        envs: Optional[dict] = None,
        timeout: int = 60,
    ) -> PtyHandle:
        """Spawn a new PTY session on the remote computer-server.

        Args:
            command: Shell command (defaults to ``bash`` on the server side).
            cols: Terminal width.
            rows: Terminal height.
            on_data: Callback invoked with raw bytes whenever the PTY produces
                output.  Called from the asyncio event loop.
            cwd: Working directory on the remote host.
            envs: Extra environment variables for the remote process.
            timeout: HTTP request timeout in seconds.

        Returns:
            :class:`PtyHandle` for the new session.
        """
        import aiohttp

        body: dict = {"cols": cols, "rows": rows}
        if command is not None:
            body["command"] = command
        if cwd is not None:
            body["cwd"] = cwd
        if envs is not None:
            body["envs"] = envs

        async with aiohttp.ClientSession() as http:
            async with http.post(
                f"{self._base_url}/pty",
                json=body,
                headers=self._auth_headers(),
                timeout=aiohttp.ClientTimeout(total=timeout),
            ) as resp:
                resp.raise_for_status()
                data = await resp.json()

        pid: int = data["pid"]
        logger.debug("PTY session created: pid=%d", pid)

        # Set up session state
        exit_event: asyncio.Event = asyncio.Event()
        exit_code_cell: list[int] = [0]
        self._sessions[pid] = {
            "exit_event": exit_event,
            "exit_code": exit_code_cell,
            "ws_task": None,
        }

        # Open WebSocket for bidirectional I/O
        ws_task = asyncio.create_task(
            self._ws_reader(pid, on_data, exit_event, exit_code_cell),
            name=f"pty-ws-{pid}",
        )
        self._sessions[pid]["ws_task"] = ws_task

        return PtyHandle(pid=pid, cols=data["cols"], rows=data["rows"], _iface=self)

    async def kill(self, pid: int) -> bool:
        """Kill the remote PTY session *pid*."""
        import aiohttp

        async with aiohttp.ClientSession() as http:
            async with http.delete(
                f"{self._base_url}/pty/{pid}",
                headers=self._auth_headers(),
                timeout=aiohttp.ClientTimeout(total=10),
            ) as resp:
                resp.raise_for_status()
                data = await resp.json()
        return bool(data.get("killed"))

    async def resize(self, pid: int, cols: int, rows: int) -> None:
        """Resize the terminal for the remote PTY session *pid*."""
        import aiohttp

        async with aiohttp.ClientSession() as http:
            async with http.post(
                f"{self._base_url}/pty/{pid}/resize",
                json={"cols": cols, "rows": rows},
                headers=self._auth_headers(),
                timeout=aiohttp.ClientTimeout(total=10),
            ) as resp:
                resp.raise_for_status()

    async def send_stdin(self, pid: int, data: bytes) -> None:
        """Write *data* to the remote PTY session's stdin via the WebSocket.

        If a WebSocket is active the data is sent through it; otherwise falls
        back to the HTTP ``/stdin`` endpoint.
        """
        sess = self._sessions.get(pid)
        ws_task = sess.get("ws_task") if sess else None

        if ws_task and not ws_task.done():
            # Route through the WS task via a shared queue
            q = sess.get("stdin_queue")
            if q is not None:
                await q.put(data)
                return

        # Fallback: HTTP POST
        import aiohttp

        async with aiohttp.ClientSession() as http:
            async with http.post(
                f"{self._base_url}/pty/{pid}/stdin",
                json={"data": base64.b64encode(data).decode()},
                headers=self._auth_headers(),
                timeout=aiohttp.ClientTimeout(total=10),
            ) as resp:
                resp.raise_for_status()

    async def connect(
        self,
        pid: int,
        on_data: Optional[Callable[[bytes], None]] = None,
    ) -> PtyHandle:
        """Re-open a WebSocket to an existing PTY session *pid*.

        Any previous connection for this pid is cancelled first.
        The current terminal dimensions are fetched from the server so that
        the returned :class:`PtyHandle` reflects any resizes since creation.
        """
        import aiohttp

        sess = self._sessions.get(pid)
        if sess:
            ws_task = sess.get("ws_task")
            if ws_task and not ws_task.done():
                ws_task.cancel()
                try:
                    await ws_task
                except (asyncio.CancelledError, Exception):
                    pass

        # Fetch current dimensions from the server.
        cols, rows = 80, 24
        try:
            async with aiohttp.ClientSession() as http:
                async with http.get(
                    f"{self._base_url}/pty/{pid}",
                    headers=self._auth_headers(),
                    timeout=aiohttp.ClientTimeout(total=10),
                ) as resp:
                    if resp.status == 200:
                        info = await resp.json()
                        cols = int(info.get("cols", cols))
                        rows = int(info.get("rows", rows))
        except Exception as exc:
            logger.debug("PTY connect: could not fetch dimensions for pid %d: %s", pid, exc)

        exit_event: asyncio.Event = asyncio.Event()
        exit_code_cell: list[int] = [0]
        self._sessions[pid] = {
            "exit_event": exit_event,
            "exit_code": exit_code_cell,
            "ws_task": None,
        }

        ws_task = asyncio.create_task(
            self._ws_reader(pid, on_data, exit_event, exit_code_cell),
            name=f"pty-ws-reconnect-{pid}",
        )
        self._sessions[pid]["ws_task"] = ws_task

        return PtyHandle(pid=pid, cols=cols, rows=rows, _iface=self)

    # ------------------------------------------------------------------
    # Internal helpers
    # ------------------------------------------------------------------

    async def _disconnect(self, pid: int) -> None:
        """Cancel the WS reader task without killing the remote process."""
        sess = self._sessions.get(pid)
        if not sess:
            return
        ws_task = sess.get("ws_task")
        if ws_task and not ws_task.done():
            ws_task.cancel()
            try:
                await ws_task
            except (asyncio.CancelledError, Exception):
                pass

    async def _wait(self, pid: int) -> int:
        """Wait for the exit event and return the exit code.

        Raises:
            LookupError: If *pid* is not a tracked session.
        """
        sess = self._sessions.get(pid)
        if not sess:
            raise LookupError(f"PTY session {pid} is not tracked by this interface")
        await sess["exit_event"].wait()
        return sess["exit_code"][0]

    async def _ws_reader(
        self,
        pid: int,
        on_data: Optional[Callable[[bytes], None]],
        exit_event: asyncio.Event,
        exit_code_cell: list[int],
    ) -> None:
        """Background coroutine: connect to the WS and forward messages."""
        import aiohttp

        stdin_queue: asyncio.Queue[bytes] = asyncio.Queue()
        sess = self._sessions.setdefault(pid, {})
        sess["stdin_queue"] = stdin_queue

        params = {}
        if self._api_key:
            params["api_key"] = self._api_key
        if self._vm_name:
            params["container_name"] = self._vm_name
        ws_url = f"{self._ws_base}/pty/{pid}/ws"
        try:
            async with aiohttp.ClientSession() as http:
                async with http.ws_connect(ws_url, params=params) as ws:

                    async def _write_stdin():
                        while True:
                            data = await stdin_queue.get()
                            payload = json.dumps(
                                {
                                    "type": "stdin",
                                    "data": base64.b64encode(data).decode(),
                                }
                            )
                            await ws.send_str(payload)

                    writer_task = asyncio.create_task(_write_stdin())

                    try:
                        async for raw_msg in ws:
                            if raw_msg.type == aiohttp.WSMsgType.TEXT:
                                try:
                                    msg = json.loads(raw_msg.data)
                                except json.JSONDecodeError:
                                    continue
                                if msg.get("type") == "output":
                                    chunk = base64.b64decode(msg["data"])
                                    if on_data is not None:
                                        on_data(chunk)
                                elif msg.get("type") == "exit":
                                    exit_code_cell[0] = int(msg.get("code", 0))
                                    break
                            elif raw_msg.type in (
                                aiohttp.WSMsgType.CLOSED,
                                aiohttp.WSMsgType.ERROR,
                            ):
                                break
                    finally:
                        writer_task.cancel()
                        try:
                            await writer_task
                        except (asyncio.CancelledError, Exception):
                            pass

        except asyncio.CancelledError:
            pass
        except Exception as exc:
            logger.debug("PTY WS reader error for pid %d: %s", pid, exc)
        finally:
            exit_event.set()

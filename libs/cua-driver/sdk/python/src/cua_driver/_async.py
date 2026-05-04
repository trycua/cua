"""Async layer for cua_driver.

Provides ``AsyncApp``, ``AsyncWindow``, ``AsyncLocator``, and
``async_expect()`` — thin ``asyncio.to_thread`` wrappers around the sync
counterparts so that every blocking call is moved off the event loop.

Also provides ``AsyncDriverClient`` — a fully asyncio-native subprocess
client for use in async test suites.

Why two approaches?
-------------------
* ``AsyncDriverClient`` is used in truly async fixtures; it owns the
  subprocess via ``asyncio.create_subprocess_exec``.
* ``AsyncApp`` / ``AsyncWindow`` / ``AsyncLocator`` just delegate to the
  sync implementations via ``asyncio.to_thread``, avoiding code
  duplication of all AX-tree parsing logic.

Usage::

    from cua_driver import AsyncApp, DriverClient, async_expect

    async def test_calc(cua_driver):          # cua_driver is sync DriverClient
        async with AsyncApp.launch("com.apple.calculator", cua_driver) as app:
            window = await app.main_window()
            await window.locator("AXButton", label="2").click()
            await window.locator("AXButton", label="Add").click()
            await window.locator("AXButton", label="2").click()
            await window.locator("AXButton", label="Equals").click()
            await async_expect(window.locator("AXStaticText")).to_contain_text("4")
"""

from __future__ import annotations

import asyncio
import json
import time
from typing import Optional, Any

from ._client import DriverClient, MCPCallError, default_binary_path
from ._app import App, _resolve_window_id
from ._window import Window
from ._locator import Locator
from ._expect import LocatorAssertions


# ---------------------------------------------------------------------------
# AsyncDriverClient — fully asyncio-native subprocess client
# ---------------------------------------------------------------------------

class AsyncDriverClient:
    """Async MCP stdio client backed by ``asyncio.create_subprocess_exec``.

    Use as an async context manager::

        async with AsyncDriverClient() as c:
            result = await c.call_tool("list_apps")

    Or manage manually::

        c = AsyncDriverClient()
        await c.start()
        try:
            result = await c.call_tool("list_apps")
        finally:
            await c.close()
    """

    def __init__(
        self,
        binary_path: Optional[str] = None,
        subcommand: str = "mcp",
    ) -> None:
        self.binary_path = binary_path or default_binary_path()
        self.subcommand = subcommand
        self._process: Optional[asyncio.subprocess.Process] = None
        self._next_id = 0
        self._pending: dict[int, asyncio.Future] = {}
        self._reader_task: Optional[asyncio.Task] = None

    async def __aenter__(self) -> "AsyncDriverClient":
        await self.start()
        return self

    async def __aexit__(self, *exc: Any) -> None:
        await self.close()

    async def start(self) -> None:
        """Spawn cua-driver and perform the MCP handshake."""
        self._process = await asyncio.create_subprocess_exec(
            self.binary_path, self.subcommand,
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        self._reader_task = asyncio.create_task(self._read_loop())
        await self._rpc("initialize", {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "cua-driver-sdk-async", "version": "0.1.0"},
        })
        self._notify("notifications/initialized")

    async def close(self) -> None:
        """Terminate the cua-driver process and cancel pending calls."""
        if self._reader_task:
            self._reader_task.cancel()
            try:
                await self._reader_task
            except asyncio.CancelledError:
                pass
        for fut in self._pending.values():
            if not fut.done():
                fut.cancel()
        self._pending.clear()
        if self._process:
            try:
                self._process.terminate()
                await asyncio.wait_for(self._process.wait(), timeout=3.0)
            except (asyncio.TimeoutError, ProcessLookupError):
                with contextlib.suppress(ProcessLookupError):
                    self._process.kill()

    async def list_tools(self) -> list[dict]:
        return (await self._rpc("tools/list"))["tools"]

    async def call_tool(
        self,
        name: str,
        arguments: Optional[dict] = None,
        timeout: float = 30.0,
    ) -> dict:
        return await self._rpc(
            "tools/call",
            {"name": name, "arguments": arguments or {}},
            timeout=timeout,
        )

    # ------------------------------------------------------------------
    # Internal
    # ------------------------------------------------------------------

    async def _read_loop(self) -> None:
        assert self._process and self._process.stdout
        try:
            async for raw in self._process.stdout:
                line = raw.decode().strip()
                if not line:
                    continue
                try:
                    msg = json.loads(line)
                except json.JSONDecodeError:
                    continue
                msg_id = msg.get("id")
                if msg_id is not None:
                    fut = self._pending.pop(msg_id, None)
                    if fut and not fut.done():
                        if "error" in msg:
                            fut.set_exception(MCPCallError(msg["error"]))
                        else:
                            fut.set_result(msg["result"])
        except asyncio.CancelledError:
            pass

    def _notify(self, method: str, params: Optional[dict] = None) -> None:
        payload: dict = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            payload["params"] = params
        self._write(payload)

    async def _rpc(
        self,
        method: str,
        params: Optional[dict] = None,
        timeout: float = 20.0,
    ) -> dict:
        loop = asyncio.get_event_loop()
        self._next_id += 1
        request_id = self._next_id
        fut: asyncio.Future = loop.create_future()
        self._pending[request_id] = fut
        payload: dict = {"jsonrpc": "2.0", "id": request_id, "method": method}
        if params is not None:
            payload["params"] = params
        self._write(payload)
        return await asyncio.wait_for(fut, timeout=timeout)

    def _write(self, payload: dict) -> None:
        assert self._process and self._process.stdin
        data = (json.dumps(payload) + "\n").encode()
        self._process.stdin.write(data)


# contextlib import at top of module would be cleaner; added here to avoid
# touching the existing imports above.
import contextlib  # noqa: E402


# ---------------------------------------------------------------------------
# AsyncApp — wraps sync App via asyncio.to_thread
# ---------------------------------------------------------------------------

class AsyncApp:
    """Async handle to a running macOS app.

    Wraps :class:`~cua_driver.App` — all blocking calls are executed in a
    thread pool via :func:`asyncio.to_thread`.

    Use as an async context manager::

        async with AsyncApp.launch("com.apple.calculator", driver) as app:
            window = await app.main_window()
            ...

    *driver* may be a sync :class:`~cua_driver.DriverClient` **or** an
    :class:`AsyncDriverClient` (both expose ``call_tool``).
    """

    def __init__(self, sync_app: App) -> None:
        self._app = sync_app

    @classmethod
    async def launch(
        cls,
        bundle_id: str,
        client: DriverClient,
        settle: float = 1.0,
    ) -> "AsyncApp":
        """Launch *bundle_id* and return an async handle."""
        app = await asyncio.to_thread(App.launch, bundle_id, client, settle)
        return cls(app)

    async def main_window(self, timeout: float = 5.0) -> "AsyncWindow":
        """Return the best available :class:`AsyncWindow`."""
        window = await asyncio.to_thread(self._app.main_window, timeout)
        return AsyncWindow(window)

    async def get_window(
        self,
        title: Optional[str] = None,
        timeout: float = 5.0,
    ) -> "AsyncWindow":
        """Return an :class:`AsyncWindow` whose title contains *title*."""
        window = await asyncio.to_thread(self._app.get_window, title, timeout)
        return AsyncWindow(window)

    async def quit(self) -> None:
        await asyncio.to_thread(self._app.quit)

    async def force_quit(self) -> None:
        await asyncio.to_thread(self._app.force_quit)

    async def __aenter__(self) -> "AsyncApp":
        return self

    async def __aexit__(self, *exc: object) -> None:
        await self.quit()

    @property
    def pid(self) -> int:
        return self._app.pid

    @property
    def bundle_id(self) -> str:
        return self._app.bundle_id


# ---------------------------------------------------------------------------
# AsyncWindow — wraps sync Window
# ---------------------------------------------------------------------------

class AsyncWindow:
    """Async handle to a specific macOS window."""

    def __init__(self, sync_window: Window) -> None:
        self._window = sync_window

    def locator(
        self,
        role: Optional[str] = None,
        *,
        label: Optional[str] = None,
        title: Optional[str] = None,
        text: Optional[str] = None,
        value: Optional[str] = None,
    ) -> "AsyncLocator":
        """Return an :class:`AsyncLocator` for elements in this window."""
        return AsyncLocator(self._window.locator(
            role, label=label, title=title, text=text, value=value,
        ))

    async def screenshot(self) -> bytes:
        return await asyncio.to_thread(self._window.screenshot)

    async def get_tree(self, query: Optional[str] = None) -> str:
        return await asyncio.to_thread(self._window.get_tree, query)

    @property
    def pid(self) -> int:
        return self._window.pid

    @property
    def window_id(self) -> int:
        return self._window.window_id

    @property
    def title(self) -> str:
        # Property access is sync but fast (list_windows call)
        return self._window.title


# ---------------------------------------------------------------------------
# AsyncLocator — wraps sync Locator
# ---------------------------------------------------------------------------

class AsyncLocator:
    """Async element locator with auto-waiting.

    All action and query methods are coroutines backed by the sync
    :class:`~cua_driver.Locator` running in a thread pool.
    """

    def __init__(self, sync_locator: Locator) -> None:
        self._locator = sync_locator

    def nth(self, n: int) -> "AsyncLocator":
        """Return a new :class:`AsyncLocator` for the *n*-th match."""
        return AsyncLocator(self._locator.nth(n))

    # Actions

    async def click(self, timeout: float = 5.0) -> None:
        await asyncio.to_thread(self._locator.click, timeout)

    async def fill(self, text: str, timeout: float = 5.0) -> None:
        await asyncio.to_thread(self._locator.fill, text, timeout)

    async def type(self, text: str, timeout: float = 5.0) -> None:
        await asyncio.to_thread(self._locator.type, text, timeout)

    async def set_value(self, value: str, timeout: float = 5.0) -> None:
        await asyncio.to_thread(self._locator.set_value, value, timeout)

    async def double_click(self, timeout: float = 5.0) -> None:
        await asyncio.to_thread(self._locator.double_click, timeout)

    # Queries

    async def text_content(self, timeout: float = 5.0) -> str:
        return await asyncio.to_thread(self._locator.text_content, timeout)

    async def all_text_contents(self) -> list[str]:
        return await asyncio.to_thread(self._locator.all_text_contents)

    async def get_value(self, timeout: float = 5.0) -> str:
        return await asyncio.to_thread(self._locator.get_value, timeout)

    async def is_visible(self, timeout: float = 0.0) -> bool:
        return await asyncio.to_thread(self._locator.is_visible, timeout)


# ---------------------------------------------------------------------------
# AsyncLocatorAssertions / async_expect
# ---------------------------------------------------------------------------

class AsyncLocatorAssertions:
    """Async assertion interface for :class:`AsyncLocator`.

    Obtain via :func:`async_expect`::

        await async_expect(window.locator("AXStaticText")).to_contain_text("4")
    """

    def __init__(self, locator: AsyncLocator, timeout: float = 5.0) -> None:
        self._locator = locator
        self._timeout = timeout

    async def to_be_visible(self, timeout: Optional[float] = None) -> None:
        t = timeout if timeout is not None else self._timeout
        deadline = asyncio.get_event_loop().time() + t
        while True:
            if await self._locator.is_visible(0.0):
                return
            if asyncio.get_event_loop().time() >= deadline:
                raise AssertionError(f"Element not visible after {t}s")
            await asyncio.sleep(0.3)

    async def not_to_be_visible(self, timeout: Optional[float] = None) -> None:
        t = timeout if timeout is not None else self._timeout
        deadline = asyncio.get_event_loop().time() + t
        while True:
            if not await self._locator.is_visible(0.0):
                return
            if asyncio.get_event_loop().time() >= deadline:
                raise AssertionError(f"Element still visible after {t}s")
            await asyncio.sleep(0.3)

    async def to_contain_text(self, text: str, timeout: Optional[float] = None) -> None:
        t = timeout if timeout is not None else self._timeout
        deadline = asyncio.get_event_loop().time() + t
        last = "<not found>"
        while True:
            contents = await self._locator.all_text_contents()
            for content in contents:
                if text in content:
                    return
            if contents:
                last = repr(contents)
            if asyncio.get_event_loop().time() >= deadline:
                raise AssertionError(
                    f"Expected text {text!r} not found in {last} after {t}s"
                )
            await asyncio.sleep(0.3)

    async def to_have_text(self, text: str, timeout: Optional[float] = None) -> None:
        t = timeout if timeout is not None else self._timeout
        deadline = asyncio.get_event_loop().time() + t
        last = "<not found>"
        while True:
            contents = await self._locator.all_text_contents()
            for content in contents:
                if content == text:
                    return
            if contents:
                last = repr(contents)
            if asyncio.get_event_loop().time() >= deadline:
                raise AssertionError(
                    f"Expected text {text!r} but got {last} after {t}s"
                )
            await asyncio.sleep(0.3)

    async def to_have_value(self, value: str, timeout: Optional[float] = None) -> None:
        t = timeout if timeout is not None else self._timeout
        deadline = asyncio.get_event_loop().time() + t
        last = "<not found>"
        while True:
            try:
                val = await self._locator.get_value(0.0)
                if val == value:
                    return
                last = val
            except TimeoutError:
                pass
            if asyncio.get_event_loop().time() >= deadline:
                raise AssertionError(
                    f"Expected value {value!r} but got {last!r} after {t}s"
                )
            await asyncio.sleep(0.3)


def async_expect(
    locator: "AsyncLocator | Locator",
    timeout: float = 5.0,
) -> AsyncLocatorAssertions:
    """Return async assertions for *locator*.

    Accepts both :class:`AsyncLocator` and sync :class:`~cua_driver.Locator`
    (auto-wraps the latter).

    Usage::

        await async_expect(window.locator("AXStaticText")).to_contain_text("4")
    """
    if isinstance(locator, Locator):
        locator = AsyncLocator(locator)
    return AsyncLocatorAssertions(locator, timeout)

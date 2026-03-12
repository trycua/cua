"""Sandbox class — the primary entry point for sandboxed environments.

Exposes .mouse, .keyboard, .screen, .clipboard, .shell, .window, .terminal
as interface objects backed by a Transport.

Usage::

    from cua_sandbox import sandbox, Image
    from cua_sandbox.runtime import DockerRuntime

    # Localhost (no VM, direct host control)
    async with sandbox(local=True) as sb:
        await sb.screen.screenshot()

    # Spin up a container from an Image spec
    async with sandbox(image=Image.linux(), runtime=DockerRuntime()) as sb:
        result = await sb.shell.run("uname -a")

    # Persistent
    sb = await Sandbox.create(local=True)
    await sb.mouse.click(100, 200)
    await sb.close()
"""

from __future__ import annotations

import asyncio
from contextlib import asynccontextmanager
from typing import Any, AsyncIterator, Optional, TYPE_CHECKING

from cua_sandbox.image import Image
from cua_sandbox.interfaces import Clipboard, Keyboard, Mouse, Screen, Shell, Terminal, Window
from cua_sandbox.transport.base import Transport
from cua_sandbox.transport.http import HTTPTransport
from cua_sandbox.transport.local import LocalTransport
from cua_sandbox.transport.websocket import WebSocketTransport

if TYPE_CHECKING:
    from cua_sandbox.runtime.base import Runtime, RuntimeInfo


def _auto_runtime(image: Image) -> "Runtime":
    """Pick a runtime automatically based on image.os_type and image.kind.

    If kind is None (unresolved registry image), the runtime must be provided
    explicitly or resolved after pulling the image manifest.
    """
    import platform as _plat

    if image.kind is None:
        raise ValueError(
            "Cannot auto-select runtime: image kind is unresolved. "
            "Either use Image.linux()/windows()/macos() which set kind automatically, "
            "or pass runtime= explicitly for registry images."
        )

    if image.kind == "container":
        from cua_sandbox.runtime.docker import DockerRuntime
        return DockerRuntime(ephemeral=True)

    # kind == "vm"
    if image.os_type == "macos":
        from cua_sandbox.runtime.lume import LumeRuntime
        return LumeRuntime()

    if image.os_type == "windows" and _plat.system() == "Windows":
        from cua_sandbox.runtime.hyperv import _has_hyperv
        if _has_hyperv():
            from cua_sandbox.runtime.hyperv import HyperVRuntime
            return HyperVRuntime()

    # Linux VM or Windows VM on non-Windows host → QEMU
    from cua_sandbox.runtime.qemu import QEMURuntime
    return QEMURuntime(mode="docker")


class Sandbox:
    """A sandboxed computer environment with interface accessors."""

    def __init__(
        self,
        transport: Transport,
        name: Optional[str] = None,
        _runtime: Optional[Runtime] = None,
        _runtime_info: Optional[RuntimeInfo] = None,
    ):
        self._transport = transport
        self.name = name
        self._runtime = _runtime
        self._runtime_info = _runtime_info
        self.screen = Screen(transport)
        self.mouse = Mouse(transport)
        self.keyboard = Keyboard(transport)
        self.clipboard = Clipboard(transport)
        self.shell = Shell(transport)
        self.window = Window(transport)
        self.terminal = Terminal(transport)

    async def _connect(self) -> None:
        await self._transport.connect()

    async def close(self) -> None:
        """Disconnect transport, then stop the runtime if we own one."""
        await self._transport.disconnect()
        if self._runtime and self._runtime_info:
            await self._runtime.stop(self._runtime_info.name or self.name or "cua-sandbox")

    async def screenshot(self, text: Optional[str] = None) -> bytes:
        return await self._transport.screenshot()

    async def screenshot_base64(self, text: Optional[str] = None) -> str:
        return await self.screen.screenshot_base64()

    async def get_environment(self) -> str:
        return await self._transport.get_environment()

    async def get_dimensions(self) -> tuple[int, int]:
        return await self.screen.size()

    # ── Async context manager ────────────────────────────────────────────

    async def __aenter__(self) -> Sandbox:
        await self._connect()
        return self

    async def __aexit__(self, *exc: Any) -> None:
        await self.close()

    # ── Factory ──────────────────────────────────────────────────────────

    @classmethod
    async def create(
        cls,
        *,
        local: bool = False,
        ws_url: Optional[str] = None,
        http_url: Optional[str] = None,
        api_key: Optional[str] = None,
        container_name: Optional[str] = None,
        image: Optional[Image] = None,
        runtime: Optional[Runtime] = None,
        name: Optional[str] = None,
    ) -> Sandbox:
        """Create and connect a persistent Sandbox.

        Args:
            local: Use LocalTransport (direct host control via cua_auto).
            ws_url: WebSocket URL for a remote computer-server.
            http_url: HTTP base URL for a remote computer-server (REST/SSE fallback).
            api_key: API key for remote connections.
            container_name: Container name for cloud auth (HTTP transport).
            image: Image spec — requires a runtime to spin up a VM/container.
            runtime: Runtime backend (DockerRuntime, QEMURuntime, LumeRuntime, HyperVRuntime).
            name: Name for the sandbox / VM / container.
        """
        rt_info = None
        if image and image.kind is None and image._registry:
            from cua_sandbox.registry.resolve import resolve_image_kind
            image = resolve_image_kind(image)
        if image and not runtime:
            runtime = _auto_runtime(image)
        if image and runtime:
            sb_name = name or "cua-sandbox"
            rt_info = await runtime.start(image, sb_name)
            transport = HTTPTransport(
                f"http://{rt_info.host}:{rt_info.api_port}",
                api_key=api_key,
                container_name=container_name,
            )
        else:
            transport = _make_transport(
                local=local, ws_url=ws_url, http_url=http_url,
                api_key=api_key, container_name=container_name,
            )
        sb = cls(transport, name=name, _runtime=runtime, _runtime_info=rt_info)
        await sb._connect()
        return sb

    def __repr__(self) -> str:
        tname = type(self._transport).__name__
        return f"Sandbox(name={self.name!r}, transport={tname})"


def _make_transport(
    *,
    local: bool = False,
    ws_url: Optional[str] = None,
    http_url: Optional[str] = None,
    api_key: Optional[str] = None,
    container_name: Optional[str] = None,
) -> Transport:
    if local:
        return LocalTransport()
    if ws_url:
        return WebSocketTransport(ws_url, api_key=api_key)
    if http_url:
        return HTTPTransport(http_url, api_key=api_key, container_name=container_name)
    raise ValueError("Must specify local=True, ws_url, http_url, or image+runtime")


@asynccontextmanager
async def sandbox(
    *,
    local: bool = False,
    ws_url: Optional[str] = None,
    http_url: Optional[str] = None,
    api_key: Optional[str] = None,
    container_name: Optional[str] = None,
    image: Optional[Image] = None,
    runtime: Optional["Runtime"] = None,
    name: Optional[str] = None,
) -> AsyncIterator[Sandbox]:
    """Async context manager that yields a Sandbox.

    Usage::

        # Direct host control
        async with sandbox(local=True) as sb:
            await sb.mouse.click(100, 200)

        # Spin up a Linux container
        async with sandbox(image=Image.linux(), runtime=DockerRuntime()) as sb:
            await sb.shell.run("whoami")
    """
    sb = await Sandbox.create(
        local=local, ws_url=ws_url, http_url=http_url,
        api_key=api_key, container_name=container_name,
        image=image, runtime=runtime, name=name,
    )
    try:
        yield sb
    finally:
        await sb.close()

"""Sandbox class — the primary entry point for sandboxed environments.

Exposes .mouse, .keyboard, .screen, .clipboard, .shell, .window, .terminal
as interface objects backed by a Transport.

Usage::

    from cua_sandbox import sandbox, Image

    # Cloud VM (default — create and destroy on exit)
    async with sandbox(image=Image.linux()) as sb:
        await sb.shell.run("uname -a")

    # Cloud VM (connect to existing, not destroyed on exit)
    async with sandbox(name="my-vm") as sb:
        await sb.screenshot()

    # Local VM via QEMU (sandboxed, not cloud)
    async with sandbox(local=True, image=Image.linux()) as sb:
        result = await sb.shell.run("uname -a")
"""

from __future__ import annotations

from contextlib import asynccontextmanager
from typing import TYPE_CHECKING, Any, AsyncIterator, Optional

from cua_sandbox.image import Image
from cua_sandbox.interfaces import (
    Clipboard,
    Keyboard,
    Mouse,
    Screen,
    Shell,
    Terminal,
    Window,
)
from cua_sandbox.transport.base import Transport
from cua_sandbox.transport.cloud import CloudTransport
from cua_sandbox.transport.http import HTTPTransport
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
    """A sandboxed computer environment.

    Provides programmatic control of a VM or container through a unified
    interface: ``.mouse``, ``.keyboard``, ``.screen``, ``.clipboard``,
    ``.shell``, ``.window``, and ``.terminal``.

    Sandboxes are always isolated — they never control the host machine
    directly. For unsandboxed host control, use :func:`cua_sandbox.localhost`.

    There are two ways to obtain a Sandbox:

    1. **Context manager** (recommended for ephemeral use)::

           async with sandbox(image=Image.linux()) as sb:
               await sb.shell.run("whoami")

    2. **Factory method** (for persistent / long-lived sessions)::

           sb = await Sandbox.create(name="my-vm")
           await sb.shell.run("whoami")
           await sb.close()

    By default, sandboxes created with ``image=`` are ephemeral — the VM is
    destroyed when the context manager exits or ``close()`` is called. Sandboxes
    connected by ``name=`` are not destroyed. Override with ``ephemeral=True``
    or ``ephemeral=False``.
    """

    def __init__(
        self,
        transport: Transport,
        name: Optional[str] = None,
        _runtime: Optional[Runtime] = None,
        _runtime_info: Optional[RuntimeInfo] = None,
        _ephemeral: Optional[bool] = None,
    ):
        self._transport = transport
        self.name = name
        self._runtime = _runtime
        self._runtime_info = _runtime_info
        self._ephemeral = _ephemeral
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
        """Disconnect transport. If ephemeral, destroy the VM/container."""
        await self._transport.disconnect()
        if self._ephemeral and isinstance(self._transport, CloudTransport):
            await self._transport.delete_vm()
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
        ephemeral: Optional[bool] = None,
        configuration: str = "small",
        region: str = "us-east-1",
    ) -> Sandbox:
        """Create and connect a persistent Sandbox.

        Args:
            local: Use a local runtime (QEMU, Docker, Lume) instead of cloud.
                   Requires image + runtime (or auto-selects runtime from image).
            ws_url: WebSocket URL for a remote computer-server.
            http_url: HTTP base URL for a remote computer-server (REST/SSE fallback).
            api_key: API key for cloud connections.
            container_name: Container name for cloud auth (HTTP transport).
            image: Image spec — used for cloud VM creation or local runtime.
            runtime: Runtime backend (DockerRuntime, QEMURuntime, LumeRuntime, HyperVRuntime).
                     If omitted with image and not local, defaults to cloud.
            name: Name for the sandbox / VM / container.
            ephemeral: Whether to destroy the VM on close. None (default) infers:
                       True when creating a new VM (image=...), False when connecting
                       to an existing one (name=...).
            configuration: Cloud VM size (default "small").
            region: Cloud VM region (default "us-east-1").
        """
        # Infer ephemeral: True when creating (image provided), False when connecting (name only)
        if ephemeral is None:
            ephemeral = bool(image)

        rt_info = None
        if image and image.kind is None and image._registry:
            from cua_sandbox.registry.resolve import resolve_image_kind

            image = resolve_image_kind(image)
        if image and not runtime and not local:
            # image without runtime and not local → cloud creation
            if not any([ws_url, http_url]):
                transport = CloudTransport(
                    name=name,
                    api_key=api_key,
                    image=image,
                    configuration=configuration,
                    region=region,
                )
                sb = cls(transport, name=name, _ephemeral=ephemeral)
                await sb._connect()
                return sb
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
                ws_url=ws_url,
                http_url=http_url,
                api_key=api_key,
                container_name=container_name,
                name=name,
                configuration=configuration,
                region=region,
            )
        sb = cls(
            transport, name=name, _runtime=runtime, _runtime_info=rt_info, _ephemeral=ephemeral
        )
        await sb._connect()
        return sb

    def __repr__(self) -> str:
        tname = type(self._transport).__name__
        return f"Sandbox(name={self.name!r}, transport={tname})"


def _make_transport(
    *,
    ws_url: Optional[str] = None,
    http_url: Optional[str] = None,
    api_key: Optional[str] = None,
    container_name: Optional[str] = None,
    name: Optional[str] = None,
    configuration: str = "small",
    region: str = "us-east-1",
) -> Transport:
    if ws_url:
        return WebSocketTransport(ws_url, api_key=api_key)
    if http_url:
        return HTTPTransport(http_url, api_key=api_key, container_name=container_name)
    # Default: cloud transport — connects by name, or errors on missing image/API key
    return CloudTransport(
        name=name,
        api_key=api_key,
        configuration=configuration,
        region=region,
    )


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
    ephemeral: Optional[bool] = None,
    configuration: str = "small",
    region: str = "us-east-1",
) -> AsyncIterator[Sandbox]:
    """Create a sandboxed VM and yield it as an async context manager.

    The sandbox is always isolated — it never controls the host machine.
    For unsandboxed host control, use :func:`cua_sandbox.localhost`.

    Args:
        local: Use a local runtime (QEMU, Docker, Lume) instead of cloud.
               Requires ``image`` (runtime is auto-selected if omitted).
        image: Image spec for VM creation. Determines the OS and VM type.
               Use ``Image.linux()``, ``Image.windows()``, or ``Image.macos()``.
        name: Connect to an existing cloud VM by name instead of creating one.
        ephemeral: Whether to destroy the VM on exit. ``None`` (default) infers
                   from context: ``True`` when creating (``image=...``), ``False``
                   when connecting by name. Override explicitly to change behavior.
        api_key: CUA API key for cloud sandboxes.
        runtime: Explicit runtime backend (DockerRuntime, QEMURuntime, etc.).
        configuration: Cloud VM size (default ``"small"``).
        region: Cloud VM region (default ``"us-east-1"``).

    Examples::

        # Cloud VM — created and destroyed on exit (ephemeral inferred True)
        async with sandbox(image=Image.linux()) as sb:
            await sb.shell.run("whoami")

        # Cloud VM — connect to existing (ephemeral inferred False)
        async with sandbox(name="my-vm") as sb:
            await sb.screenshot()

        # Cloud VM — create but keep alive after exit
        async with sandbox(image=Image.linux(), ephemeral=False) as sb:
            print(sb.name)  # save this to reconnect later

        # Local VM via QEMU (sandboxed, not cloud)
        async with sandbox(local=True, image=Image.linux()) as sb:
            await sb.shell.run("whoami")
    """
    sb = await Sandbox.create(
        local=local,
        ws_url=ws_url,
        http_url=http_url,
        api_key=api_key,
        container_name=container_name,
        image=image,
        runtime=runtime,
        name=name,
        ephemeral=ephemeral,
        configuration=configuration,
        region=region,
    )
    try:
        yield sb
    finally:
        await sb.close()

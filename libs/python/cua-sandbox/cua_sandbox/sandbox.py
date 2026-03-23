"""Sandbox class — the primary entry point for sandboxed environments.

Exposes .mouse, .keyboard, .screen, .clipboard, .shell, .window, .terminal
as interface objects backed by a Transport.

Usage::

    from cua_sandbox import Sandbox, Image

    # Provision a new persistent sandbox
    sb = await Sandbox.create(Image.desktop("ubuntu"))
    await sb.shell.run("uname -a")
    await sb.disconnect()

    # Connect to an existing sandbox by name (plain await or async with)
    sb = await Sandbox.connect("my-sandbox")
    await sb.screenshot()
    await sb.disconnect()

    async with Sandbox.connect("my-sandbox") as sb:  # disconnects on exit
        await sb.screenshot()

    # Ephemeral — auto-destroyed on exit
    async with Sandbox.ephemeral(Image.desktop("ubuntu")) as sb:
        await sb.shell.run("whoami")
"""

from __future__ import annotations

from contextlib import asynccontextmanager
from typing import (
    TYPE_CHECKING,
    Any,
    AsyncIterator,
    Callable,
    Coroutine,
    Optional,
    TypeVar,
)

from cua_sandbox.image import Image
from cua_sandbox.interfaces import (
    Clipboard,
    Keyboard,
    Mobile,
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

_T = TypeVar("_T")


class _ConnectResult:
    """Returned by connect() — supports both ``await`` and ``async with``.

    Usage::

        # plain await
        sb = await Sandbox.connect("name")

        # context manager — disconnects on exit (sandbox keeps running)
        async with Sandbox.connect("name") as sb:
            ...
    """

    __slots__ = ("_factory", "_instance")

    def __init__(self, factory: Callable[[], Coroutine[Any, Any, _T]]) -> None:
        self._factory = factory
        self._instance: Any = None

    def __await__(self) -> Any:
        return self._factory().__await__()

    async def __aenter__(self) -> Any:
        self._instance = await self._factory()
        return self._instance

    async def __aexit__(self, *exc: Any) -> None:
        if self._instance is not None:
            await self._instance.disconnect()


def _auto_runtime(image: Image) -> "Runtime":
    """Pick a runtime automatically based on image.os_type and image.kind."""
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

    if image.os_type == "android":
        from cua_sandbox.runtime.android_emulator import AndroidEmulatorRuntime

        return AndroidEmulatorRuntime()

    if image.os_type == "windows" and _plat.system() == "Windows":
        from cua_sandbox.runtime.hyperv import _has_hyperv

        if _has_hyperv():
            from cua_sandbox.runtime.hyperv import HyperVRuntime

            return HyperVRuntime()

    # If image has a disk path (from_file), use bare-metal QEMU
    if image._disk_path:
        from cua_sandbox.runtime.qemu import QEMURuntime

        return QEMURuntime(mode="bare-metal")

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

    There are three ways to obtain a Sandbox:

    1. **Persistent** — provision and keep alive after the script exits::

           sb = await Sandbox.create(Image.desktop("ubuntu"))
           await sb.shell.run("whoami")
           await sb.disconnect()

    2. **Connect** — attach to an already-running sandbox by name::

           sb = await Sandbox.connect("my-sandbox")
           await sb.screenshot()
           await sb.disconnect()

    3. **Ephemeral** — auto-destroyed when the ``async with`` block exits::

           async with Sandbox.ephemeral(Image.desktop("ubuntu")) as sb:
               await sb.shell.run("whoami")
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
        self.mobile = Mobile(transport)

    async def _connect(self) -> None:
        await self._transport.connect()
        # Update name from transport (e.g. CloudTransport resolves name after creating a VM)
        if self.name is None and isinstance(self._transport, CloudTransport):
            self.name = self._transport.name

    async def disconnect(self) -> None:
        """Drop the transport connection. The sandbox keeps running."""
        await self._transport.disconnect()

    async def destroy(self) -> None:
        """Disconnect and permanently delete the sandbox (VM/container)."""
        await self._transport.disconnect()
        if isinstance(self._transport, CloudTransport):
            await self._transport.delete_vm()
        if self._runtime and self._runtime_info:
            await self._runtime.stop(self._runtime_info.name or self.name or "cua-sandbox")

    async def screenshot(
        self, text: Optional[str] = None, format: str = "png", quality: int = 95
    ) -> bytes:
        _MAGIC: dict[bytes, str] = {b"\x89PNG": "png", b"\xff\xd8\xff": "jpeg"}
        data = await self._transport.screenshot(format=format, quality=quality)
        got_format = next(
            (fmt for magic, fmt in _MAGIC.items() if data.startswith(magic)), "unknown"
        )
        expected = "jpeg" if format.lower() in ("jpeg", "jpg") else format.lower()
        if got_format != expected:
            raise ValueError(
                f"requested {format!r} but got {got_format!r} (magic bytes: {data[:4].hex()})"
            )
        return data

    async def screenshot_base64(
        self, text: Optional[str] = None, format: str = "png", quality: int = 95
    ) -> str:
        return await self.screen.screenshot_base64(format=format, quality=quality)

    async def get_environment(self) -> str:
        return await self._transport.get_environment()

    async def get_display_url(self, *, share: bool = False) -> str:
        """Return a URL to view this sandbox's display.

        Args:
            share: If True, return a public link with embedded credentials
                   (cloud only). If False, return a direct connection URL.
        """
        return await self._transport.get_display_url(share=share)

    async def get_dimensions(self) -> tuple[int, int]:
        return await self.screen.size()

    # ── Async context manager ────────────────────────────────────────────

    async def __aenter__(self) -> Sandbox:
        await self._connect()
        return self

    async def __aexit__(self, *exc: Any) -> None:
        await self.disconnect()

    # ── Public factory methods ───────────────────────────────────────────

    @classmethod
    async def create(
        cls,
        image: Image,
        *,
        name: Optional[str] = None,
        api_key: Optional[str] = None,
        local: bool = False,
        runtime: Optional["Runtime"] = None,
        cpu: Optional[int] = None,
        memory_mb: Optional[int] = None,
        disk_gb: Optional[int] = None,
        region: str = "us-east-1",
    ) -> "Sandbox":
        """Provision a new persistent sandbox and return it connected.

        The sandbox is kept alive after your script exits — call ``close()``
        when you are done, or use :meth:`ephemeral` if you want it destroyed
        automatically.

        Args:
            image: Image to run (e.g. ``Image.desktop("ubuntu")``).
            name: Optional name to assign to the sandbox.
            api_key: CUA API key for cloud sandboxes.
            local: Use a local runtime instead of cloud.
            runtime: Explicit runtime backend (DockerRuntime, QEMURuntime, etc.).
            cpu: Number of CPUs for the cloud sandbox.
            memory_mb: Memory in MB for the cloud sandbox.
            disk_gb: Disk size in GB for the cloud sandbox.
            region: Cloud region (default ``"us-east-1"``).

        Example::

            sb = await Sandbox.create(Image.desktop("ubuntu"))
            await sb.shell.run("uname -a")
            print(sb.name)  # save to reconnect later
            await sb.disconnect()
        """
        return await cls._create(
            image=image,
            name=name,
            ephemeral=False,
            api_key=api_key,
            local=local,
            runtime=runtime,
            cpu=cpu,
            memory_mb=memory_mb,
            disk_gb=disk_gb,
            region=region,
        )

    @classmethod
    def connect(
        cls,
        name: str,
        *,
        api_key: Optional[str] = None,
        ws_url: Optional[str] = None,
        http_url: Optional[str] = None,
        container_name: Optional[str] = None,
        cpu: Optional[int] = None,
        memory_mb: Optional[int] = None,
        disk_gb: Optional[int] = None,
        region: str = "us-east-1",
    ) -> "_ConnectResult":
        """Connect to an existing sandbox by name.

        Supports both ``await`` and ``async with``. When used as a context
        manager, ``disconnect()`` is called on exit — the sandbox keeps running.

        Args:
            name: Name of the existing sandbox.
            api_key: CUA API key for cloud sandboxes.
            ws_url: WebSocket URL for a remote computer-server.
            http_url: HTTP base URL for a remote computer-server.
            container_name: Container name for cloud auth (HTTP transport).
            region: Cloud region (default ``"us-east-1"``).

        Examples::

            # plain await
            sb = await Sandbox.connect("my-sandbox")
            await sb.screenshot()
            await sb.disconnect()

            # context manager — disconnects on exit, sandbox keeps running
            async with Sandbox.connect("my-sandbox") as sb:
                await sb.screenshot()
        """

        async def _factory() -> "Sandbox":
            return await cls._create(
                name=name,
                ephemeral=False,
                api_key=api_key,
                ws_url=ws_url,
                http_url=http_url,
                container_name=container_name,
                cpu=cpu,
                memory_mb=memory_mb,
                disk_gb=disk_gb,
                region=region,
            )

        return _ConnectResult(_factory)

    @classmethod
    @asynccontextmanager
    async def ephemeral(
        cls,
        image: Image,
        *,
        name: Optional[str] = None,
        api_key: Optional[str] = None,
        local: bool = False,
        runtime: Optional["Runtime"] = None,
        cpu: Optional[int] = None,
        memory_mb: Optional[int] = None,
        disk_gb: Optional[int] = None,
        region: str = "us-east-1",
    ) -> AsyncIterator["Sandbox"]:
        """Create an ephemeral sandbox that is automatically destroyed on exit.

        Args:
            image: Image to run (e.g. ``Image.desktop("ubuntu")``).
            name: Optional name to assign to the sandbox.
            api_key: CUA API key for cloud sandboxes.
            local: Use a local runtime instead of cloud.
            runtime: Explicit runtime backend (DockerRuntime, QEMURuntime, etc.).
            cpu: Number of CPUs for the cloud sandbox.
            memory_mb: Memory in MB for the cloud sandbox.
            disk_gb: Disk size in GB for the cloud sandbox.
            region: Cloud region (default ``"us-east-1"``).

        Example::

            async with Sandbox.ephemeral(Image.desktop("ubuntu")) as sb:
                await sb.shell.run("whoami")
            # sandbox is destroyed here
        """
        sb = await cls._create(
            image=image,
            name=name,
            ephemeral=True,
            api_key=api_key,
            local=local,
            runtime=runtime,
            cpu=cpu,
            memory_mb=memory_mb,
            disk_gb=disk_gb,
            region=region,
        )
        try:
            yield sb
        finally:
            await sb.destroy()

    # ── Internal factory ─────────────────────────────────────────────────

    @classmethod
    async def _create(
        cls,
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
        cpu: Optional[int] = None,
        memory_mb: Optional[int] = None,
        disk_gb: Optional[int] = None,
        region: str = "us-east-1",
    ) -> "Sandbox":
        """Internal workhorse — all public factories delegate here."""
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
                    cpu=cpu,
                    memory_mb=memory_mb,
                    disk_gb=disk_gb,
                    region=region,
                )
                sb = cls(transport, name=name, _ephemeral=ephemeral)
                await sb._connect()
                return sb
            runtime = _auto_runtime(image)
        if image and runtime:
            sb_name = name or "cua-sandbox"
            rt_info = await runtime.start(image, sb_name)
            if rt_info.environment == "android" and not rt_info.qmp_port:
                from cua_sandbox.transport.adb import ADBTransport

                adb_serial = f"emulator-{rt_info.api_port - 1}"
                sdk_root = None
                if hasattr(runtime, "_sdk") and runtime._sdk:
                    sdk_root = str(runtime._sdk)
                transport = ADBTransport(serial=adb_serial, sdk_root=sdk_root)
            elif rt_info.agent_type == "osworld":
                from cua_sandbox.transport.osworld import OSWorldTransport

                transport = OSWorldTransport(
                    f"http://{rt_info.host}:{rt_info.api_port}",
                )
            elif rt_info.vnc_port and rt_info.ssh_port:
                from cua_sandbox.transport.vncssh import VNCSSHTransport

                await runtime.is_ready(rt_info)
                transport = VNCSSHTransport(
                    ssh_host=rt_info.host,
                    ssh_port=rt_info.ssh_port,
                    ssh_username=rt_info.ssh_username or "admin",
                    ssh_password=rt_info.ssh_password or "admin",
                    vnc_host=rt_info.vnc_host or rt_info.host,
                    vnc_port=rt_info.vnc_port,
                    vnc_password=rt_info.vnc_password,
                    environment=rt_info.environment or image.os_type,
                )
            elif rt_info.vnc_port and not rt_info.qmp_port:
                from cua_sandbox.transport.vnc import VNCTransport

                transport = VNCTransport(
                    host=rt_info.host,
                    port=rt_info.vnc_port,
                    environment=rt_info.environment or image.os_type,
                )
            elif rt_info.qmp_port:
                from cua_sandbox.transport.qmp import QMPTransport

                transport = QMPTransport(
                    qmp_host=rt_info.host,
                    qmp_port=rt_info.qmp_port,
                    environment=rt_info.environment or image.os_type,
                )
            else:
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
                cpu=cpu,
                memory_mb=memory_mb,
                disk_gb=disk_gb,
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
    cpu: Optional[int] = None,
    memory_mb: Optional[int] = None,
    disk_gb: Optional[int] = None,
    region: str = "us-east-1",
) -> Transport:
    if ws_url:
        return WebSocketTransport(ws_url, api_key=api_key)
    if http_url:
        return HTTPTransport(http_url, api_key=api_key, container_name=container_name)
    return CloudTransport(
        name=name,
        api_key=api_key,
        cpu=cpu,
        memory_mb=memory_mb,
        disk_gb=disk_gb,
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
    cpu: Optional[int] = None,
    memory_mb: Optional[int] = None,
    disk_gb: Optional[int] = None,
    region: str = "us-east-1",
) -> AsyncIterator[Sandbox]:
    """Async context manager for a sandboxed environment.

    .. deprecated::
        Prefer ``Sandbox.create()``, ``Sandbox.connect()``, or
        ``Sandbox.ephemeral()`` instead.
    """
    sb = await Sandbox._create(
        local=local,
        ws_url=ws_url,
        http_url=http_url,
        api_key=api_key,
        container_name=container_name,
        image=image,
        runtime=runtime,
        name=name,
        ephemeral=ephemeral,
        cpu=cpu,
        memory_mb=memory_mb,
        disk_gb=disk_gb,
        region=region,
    )
    try:
        yield sb
    finally:
        if sb._ephemeral:
            await sb.destroy()
        else:
            await sb.disconnect()

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

import random
import time
from contextlib import asynccontextmanager
from dataclasses import dataclass
from typing import (
    TYPE_CHECKING,
    Any,
    AsyncIterator,
    Callable,
    Coroutine,
    Optional,
    TypeVar,
)

try:
    from core.telemetry import is_telemetry_enabled, record_event

    _TELEMETRY_AVAILABLE = True
except ImportError:
    _TELEMETRY_AVAILABLE = False

    def is_telemetry_enabled() -> bool:
        return False

    def record_event(event_name: str, properties: dict | None = None) -> None:
        pass


from cua_sandbox.image import Image
from cua_sandbox.interfaces import (
    Clipboard,
    Keyboard,
    Mobile,
    Mouse,
    Screen,
    Shell,
    Terminal,
    Tunnel,
    Window,
)
from cua_sandbox.transport.base import Transport
from cua_sandbox.transport.cloud import CloudTransport
from cua_sandbox.transport.http import HTTPTransport
from cua_sandbox.transport.websocket import WebSocketTransport

if TYPE_CHECKING:
    from cua_sandbox.runtime.base import Runtime, RuntimeInfo

_T = TypeVar("_T")


@dataclass
class SandboxInfo:
    """Metadata for a local or cloud sandbox."""

    name: str
    status: str  # "running" | "suspended" | "stopped" | "provisioning"
    source: str  # "cloud" | "lume" | "docker" | "qemu-baremetal" | "qemu-docker"
    os_type: Optional[str] = None
    host: Optional[str] = None
    vnc_url: Optional[str] = None
    api_url: Optional[str] = None
    created_at: Optional[str] = None


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

    # Linux VM or Windows VM → prefer Docker-wrapped QEMU; fall back to bare-metal
    from cua_sandbox.runtime.qemu import QEMURuntime

    if image.os_type == "windows":
        # Windows bare-metal QEMU works on any host with qemu-system-x86_64
        try:
            from cua_sandbox.runtime.docker import _has_docker

            if not _has_docker():
                return QEMURuntime(mode="bare-metal")
        except Exception:
            pass

    return QEMURuntime(mode="docker")


def _record_sandbox_create(
    sb: Any,
    *,
    image: Optional[Any],
    local: bool,
    ephemeral: bool,
    t_start: float,
) -> None:
    """Fire a sandbox_create PostHog event if telemetry is enabled."""
    if not sb.telemetry_enabled or not _TELEMETRY_AVAILABLE or not is_telemetry_enabled():
        return
    props: dict = {
        "name": sb.name,
        "local": local,
        "ephemeral": ephemeral,
        "duration_seconds": round(time.monotonic() - t_start, 3),
    }
    if image is not None:
        props["os_type"] = image.os_type
        props["image_kind"] = image.kind
    if sb._runtime is not None:
        props["runtime_type"] = type(sb._runtime).__name__
    record_event("sandbox_create", props)


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
        _telemetry_enabled: bool = True,
    ):
        self._transport = transport
        self.name = name
        self._runtime = _runtime
        self._runtime_info = _runtime_info
        self._ephemeral = _ephemeral
        self._has_snapshots = False
        self.telemetry_enabled = _telemetry_enabled
        self.screen = Screen(transport)
        self.mouse = Mouse(transport)
        self.keyboard = Keyboard(transport)
        self.clipboard = Clipboard(transport)
        self.shell = Shell(transport)
        self.window = Window(transport)
        self.terminal = Terminal(transport)
        self.mobile = Mobile(transport)
        self.tunnel = Tunnel(transport)

    async def _connect(self) -> None:
        await self._transport.connect()
        # Update name from transport (e.g. CloudTransport resolves name after creating a VM)
        if self.name is None and isinstance(self._transport, CloudTransport):
            self.name = self._transport.name

    async def disconnect(self) -> None:
        """Drop the transport connection. The sandbox keeps running."""
        await self._transport.disconnect()

    async def snapshot(self, name: str | None = None, stateful: bool = False) -> "Image":
        """Snapshot this sandbox's current state. Returns an Image.

        The returned Image can be passed to Sandbox.create() or Sandbox.ephemeral()
        to boot a new sandbox from the snapshot (COW fork — instant on btrfs).

        Args:
            name: Optional human-readable name for the snapshot.
            stateful: Whether to capture memory state (VMs only).

        Returns:
            An Image with _snapshot_source set, ready to pass to Sandbox.ephemeral().
        """
        from cua_sandbox.transport.cloud import CloudTransport

        if not isinstance(self._transport, CloudTransport):
            raise NotImplementedError("Snapshots are only supported for cloud sandboxes")

        image_desc = await self._transport.create_snapshot(name=name, stateful=stateful)
        self._has_snapshots = True
        from cua_sandbox.image import Image as ImageCls

        # Get the original image from the transport for os_type/distro/version
        src_image = getattr(self._transport, "_image", None)

        # Prefer the original image's os_type/distro/version — image_desc["kind"]
        # is the snapshot kind (e.g. "vm"), not the OS type, and would misclassify
        # the image for OS-gated builder methods and compat checks.
        return ImageCls(
            os_type=src_image.os_type if src_image else image_desc.get("os_type", "linux"),
            distro=src_image.distro if src_image else image_desc.get("distro", "ubuntu"),
            version=src_image.version if src_image else image_desc.get("version", "24.04"),
            kind=src_image.kind if src_image else image_desc.get("kind"),
            _snapshot_source=image_desc,
        )

    async def destroy(self) -> None:
        """Disconnect and permanently delete the sandbox (VM/container)."""
        if self._has_snapshots:
            import logging

            logging.getLogger(__name__).warning(
                "Destroying sandbox %s which has snapshots — "
                "forks referencing those snapshots will break. "
                "Use Sandbox.ephemeral() which auto-stops instead of deleting "
                "when snapshots exist.",
                self.name,
            )
        if self.telemetry_enabled and _TELEMETRY_AVAILABLE and is_telemetry_enabled():
            record_event("sandbox_destroy", {"name": self.name, "ephemeral": self._ephemeral})
        await self._transport.disconnect()
        if isinstance(self._transport, CloudTransport):
            await self._transport.delete_vm()
        if self._runtime and self._runtime_info:
            vm_name = self._runtime_info.name or self.name or "cua-sandbox"
            if self._ephemeral and hasattr(self._runtime, "delete"):
                await self._runtime.delete(vm_name)
            else:
                await self._runtime.stop(vm_name)

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
        telemetry_enabled: bool = True,
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
            telemetry_enabled: Set to False to disable telemetry for this instance.

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
            telemetry_enabled=telemetry_enabled,
        )

    @classmethod
    def connect(
        cls,
        name: str,
        *,
        api_key: Optional[str] = None,
        local: bool = False,
        ws_url: Optional[str] = None,
        http_url: Optional[str] = None,
        container_name: Optional[str] = None,
        cpu: Optional[int] = None,
        memory_mb: Optional[int] = None,
        disk_gb: Optional[int] = None,
        region: str = "us-east-1",
        telemetry_enabled: bool = True,
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
                local=local,
                api_key=api_key,
                ws_url=ws_url,
                http_url=http_url,
                container_name=container_name,
                cpu=cpu,
                memory_mb=memory_mb,
                disk_gb=disk_gb,
                region=region,
                telemetry_enabled=telemetry_enabled,
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
        telemetry_enabled: bool = True,
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
            telemetry_enabled=telemetry_enabled,
        )
        try:
            yield sb
        finally:
            if sb._has_snapshots and sb.name:
                # Stop instead of delete so forks can reference the snapshots.
                await cls.suspend(sb.name, local=local, api_key=api_key)
            else:
                await sb.destroy()

    # ── Lifecycle management ─────────────────────────────────────────────

    @classmethod
    async def list(
        cls,
        *,
        local: bool = False,
        api_key: Optional[str] = None,
    ) -> "list[SandboxInfo]":
        """List running and suspended sandboxes.

        Args:
            local: If True, list local sandboxes (Lume, Docker, QEMU).
                   If False, list cloud sandboxes.
            api_key: CUA API key for cloud sandboxes.
        """
        if local:
            return await cls._list_local()
        return await cls._list_cloud(api_key=api_key)

    @classmethod
    async def _list_local(cls) -> "list[SandboxInfo]":
        import asyncio

        from cua_sandbox.runtime.android_emulator import AndroidEmulatorRuntime
        from cua_sandbox.runtime.docker import DockerRuntime
        from cua_sandbox.runtime.lume import LumeRuntime
        from cua_sandbox.runtime.qemu import QEMUBaremetalRuntime

        async def _list_baremetal():
            return await QEMUBaremetalRuntime().list()

        async def _list_docker():
            try:
                return await DockerRuntime().list()
            except Exception:
                return []

        async def _list_lume():
            try:
                return await LumeRuntime().list()
            except Exception:
                return []

        async def _list_android():
            try:
                return await AndroidEmulatorRuntime().list()
            except Exception:
                return []

        baremetal_vms, docker_vms, lume_vms, android_vms = await asyncio.gather(
            _list_baremetal(), _list_docker(), _list_lume(), _list_android()
        )

        results: list[SandboxInfo] = []
        for vm in baremetal_vms:
            results.append(
                SandboxInfo(
                    name=vm["name"],
                    status=vm["status"],
                    source="qemu-baremetal",
                    os_type=vm.get("os_type"),
                    host=vm.get("host"),
                    api_url=(
                        f"http://{vm['host']}:{vm['api_port']}"
                        if vm.get("host") and vm.get("api_port")
                        else None
                    ),
                )
            )
        for vm in docker_vms:
            results.append(
                SandboxInfo(
                    name=vm["name"],
                    status=vm["status"],
                    source=vm.get("runtime_type", "docker"),
                    host="localhost",
                )
            )
        for vm in lume_vms:
            results.append(
                SandboxInfo(
                    name=vm["name"],
                    status=vm["status"],
                    source="lume",
                    os_type=vm.get("os_type"),
                    host=vm.get("ip_address"),
                )
            )
        for vm in android_vms:
            results.append(
                SandboxInfo(
                    name=vm["name"],
                    status=vm["status"],
                    source="androidemulator",
                    os_type=vm.get("os_type"),
                    host=vm.get("host"),
                    api_url=(
                        f"http://{vm['host']}:{vm['api_port']}"
                        if vm.get("host") and vm.get("api_port")
                        else None
                    ),
                )
            )
        return results

    @classmethod
    async def _list_cloud(cls, *, api_key: Optional[str] = None) -> "list[SandboxInfo]":
        from cua_sandbox.transport.cloud import cloud_list_vms

        vms = await cloud_list_vms(api_key=api_key)
        results = []
        for vm in vms:
            raw_status = vm.get("status", "unknown")
            results.append(
                SandboxInfo(
                    name=vm.get("name", ""),
                    status=raw_status,
                    source="cloud",
                    os_type=vm.get("os_type") or vm.get("os"),
                    created_at=vm.get("created_at"),
                )
            )
        return results

    @classmethod
    async def get_info(
        cls,
        name: str,
        *,
        local: bool = False,
        api_key: Optional[str] = None,
    ) -> "SandboxInfo":
        """Get metadata for a specific sandbox.

        Args:
            name: Sandbox name.
            local: If True, look up in local runtimes.
            api_key: CUA API key for cloud.
        """
        if local:
            sandboxes = await cls._list_local()
            match = next((s for s in sandboxes if s.name == name), None)
            if match:
                return match
            # Fall back to state file
            from cua_sandbox import sandbox_state

            state = sandbox_state.load(name)
            if state:
                return SandboxInfo(
                    name=name,
                    status=state.get("status", "unknown"),
                    source=state.get("runtime_type", "unknown"),
                    os_type=state.get("os_type"),
                    host=state.get("host"),
                    api_url=(
                        f"http://{state['host']}:{state['api_port']}"
                        if state.get("host") and state.get("api_port")
                        else None
                    ),
                )
            raise ValueError(f"Local sandbox '{name}' not found.")
        from cua_sandbox.transport.cloud import cloud_get_vm

        vm = await cloud_get_vm(name, api_key=api_key)
        return SandboxInfo(
            name=vm.get("name", name),
            status=vm.get("status", "unknown"),
            source="cloud",
            os_type=vm.get("os_type") or vm.get("os"),
            created_at=vm.get("created_at"),
        )

    @classmethod
    async def suspend(
        cls,
        name: str,
        *,
        local: bool = False,
        api_key: Optional[str] = None,
    ) -> None:
        """Suspend a running sandbox (save state).

        For local QEMU bare-metal: saves a QMP snapshot then quits the process.
        For local Docker/QEMU-docker: pauses the container.
        For local Lume: stops the Lume VM (Lume persists state).
        For cloud: calls POST /v1/vms/{name}/stop.

        Args:
            name: Sandbox name.
            local: If True, operate on a local sandbox.
            api_key: CUA API key for cloud.
        """
        if local:
            await cls._suspend_local(name)
            return
        from cua_sandbox.transport.cloud import cloud_vm_action

        await cloud_vm_action(name, "stop", api_key=api_key)

    @classmethod
    async def _suspend_local(cls, name: str) -> None:
        from cua_sandbox import sandbox_state
        from cua_sandbox.runtime.lume import LumeRuntime

        state = sandbox_state.load(name)
        runtime_type = state.get("runtime_type") if state else None
        if runtime_type == "lume":
            await LumeRuntime().suspend(name)
        elif runtime_type == "qemu-baremetal":
            from cua_sandbox.runtime.qemu import QEMUBaremetalRuntime

            rt = QEMUBaremetalRuntime()
            if state:
                rt.qmp_port = state.get("qmp_port", rt.qmp_port)
            await rt.suspend(name)
        elif runtime_type in ("docker", "qemu-docker"):
            import subprocess

            subprocess.run(["docker", "pause", name], capture_output=True)
            sandbox_state.update(name, status="suspended")
        else:
            # Try docker pause as fallback
            import subprocess

            subprocess.run(["docker", "pause", name], capture_output=True)

    @classmethod
    async def resume(
        cls,
        name: str,
        *,
        local: bool = False,
        api_key: Optional[str] = None,
    ) -> "Sandbox":
        """Resume a suspended sandbox and return a connected Sandbox.

        Args:
            name: Sandbox name.
            local: If True, resume a local sandbox.
            api_key: CUA API key for cloud.

        Returns:
            A connected Sandbox ready to use.
        """
        if local:
            return await cls._resume_local(name)
        from cua_sandbox.transport.cloud import cloud_vm_action

        await cloud_vm_action(name, "run", api_key=api_key)
        # Connect to the now-running cloud sandbox
        sb = await cls._create(name=name, ephemeral=False, api_key=api_key)
        return sb

    @classmethod
    async def _resume_local(cls, name: str) -> "Sandbox":
        from cua_sandbox import sandbox_state
        from cua_sandbox.transport.http import HTTPTransport

        state = sandbox_state.load(name)
        if state is None:
            raise ValueError(f"No local sandbox named '{name}' found in state files.")
        runtime_type = state.get("runtime_type")
        if runtime_type == "lume":
            from cua_sandbox.image import Image
            from cua_sandbox.runtime.lume import LumeRuntime

            image = Image.from_dict(state["image"])
            rt = LumeRuntime()
            rt_info = await rt.resume(image, name)
        elif runtime_type == "qemu-baremetal":
            from cua_sandbox.image import Image
            from cua_sandbox.runtime.qemu import QEMUBaremetalRuntime

            image = Image.from_dict(state["image"])
            rt = QEMUBaremetalRuntime(
                api_port=state.get("api_port", 8000),
                vnc_display=state.get("vnc_display", 0),
                memory_mb=state.get("memory_mb", 4096),
                cpu_count=state.get("cpu_count", 2),
                arch=state.get("arch", "x86_64"),
                qmp_port=state.get("qmp_port", 4444),
            )
            rt_info = await rt.resume(image, name)
        elif runtime_type in ("docker", "qemu-docker"):
            import subprocess

            subprocess.run(["docker", "unpause", name], capture_output=True)
            api_port = state.get("api_port", 8000)
            sandbox_state.update(name, status="running")
            rt_info = None
            transport = HTTPTransport(f"http://localhost:{api_port}")
            sb = cls(transport, name=name, _ephemeral=False)
            await sb._connect()
            return sb
        else:
            raise ValueError(
                f"Cannot resume sandbox '{name}': unknown runtime_type '{runtime_type}'"
            )
        transport = HTTPTransport(f"http://{rt_info.host}:{rt_info.api_port}")
        sb = cls(transport, name=name, _ephemeral=False)
        await sb._connect()
        return sb

    @classmethod
    async def restart(
        cls,
        name: str,
        *,
        local: bool = False,
        api_key: Optional[str] = None,
    ) -> "Sandbox":
        """Restart a sandbox (suspend then resume) and return a connected Sandbox.

        Args:
            name: Sandbox name.
            local: If True, restart a local sandbox.
            api_key: CUA API key for cloud.

        Returns:
            A connected Sandbox ready to use.
        """
        if local:
            await cls._suspend_local(name)
            return await cls._resume_local(name)
        from cua_sandbox.transport.cloud import cloud_vm_action

        await cloud_vm_action(name, "restart", api_key=api_key)
        sb = await cls._create(name=name, ephemeral=False, api_key=api_key)
        return sb

    @classmethod
    async def delete(
        cls,
        name: str,
        *,
        local: bool = False,
        api_key: Optional[str] = None,
    ) -> None:
        """Permanently delete a sandbox.

        For local sandboxes, stops the VM and removes the state file.
        For cloud sandboxes, calls DELETE /v1/vms/{name}.

        Args:
            name: Sandbox name.
            local: If True, delete a local sandbox.
            api_key: CUA API key for cloud.
        """
        if local:
            await cls._delete_local(name)
            return
        from cua_sandbox.transport.cloud import cloud_vm_action

        await cloud_vm_action(name, "delete", api_key=api_key)

    @classmethod
    async def _delete_local(cls, name: str) -> None:
        from cua_sandbox import sandbox_state

        state = sandbox_state.load(name)
        runtime_type = state.get("runtime_type") if state else None
        if runtime_type == "lume":
            from cua_sandbox.runtime.lume import LumeRuntime

            await LumeRuntime().delete(name)
        elif runtime_type == "qemu-baremetal":
            from cua_sandbox.runtime.qemu import QEMUBaremetalRuntime

            await QEMUBaremetalRuntime().stop(name)  # stop() already deletes state file
            return
        elif runtime_type == "androidemulator":
            from cua_sandbox.runtime.android_emulator import AndroidEmulatorRuntime

            await AndroidEmulatorRuntime().stop(name)
        elif runtime_type in ("docker", "qemu-docker"):
            import subprocess

            subprocess.run(["docker", "stop", name], capture_output=True)
            subprocess.run(["docker", "rm", name], capture_output=True)
        sandbox_state.delete(name)

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
        telemetry_enabled: bool = True,
    ) -> "Sandbox":
        """Internal workhorse — all public factories delegate here."""
        _t_start = time.monotonic()
        if ephemeral is None:
            ephemeral = bool(image)

        rt_info = None
        if image and image.kind is None and image._registry:
            from cua_sandbox.registry.resolve import resolve_image_kind

            image = resolve_image_kind(image)

        # Local connect by name — read state file
        if name and not image and local and not ws_url and not http_url:
            from cua_sandbox import sandbox_state

            state = sandbox_state.load(name)
            if state is None:
                raise ValueError(
                    f"No local sandbox named '{name}' found. "
                    f"Check ~/.cua/sandboxes/ or create it with Sandbox.create()."
                )
            if state.get("os_type") == "android":
                grpc_port = state.get("grpc_port")
                adb_serial = state.get("adb_serial") or f"emulator-{state['api_port'] - 1}"
                sdk_root = state.get("sdk_root")
                if grpc_port:
                    from cua_sandbox.transport.grpc_emulator import (
                        GRPCEmulatorTransport,
                    )
                    from google.protobuf import empty_pb2  # noqa: F401

                    transport = GRPCEmulatorTransport(
                        host=state["host"],
                        grpc_port=grpc_port,
                        serial=adb_serial,
                        sdk_root=sdk_root,
                    )
                else:
                    from cua_sandbox.transport.adb import ADBTransport

                    transport = ADBTransport(serial=adb_serial, sdk_root=sdk_root)
            else:
                api_url = f"http://{state['host']}:{state['api_port']}"
                transport = HTTPTransport(api_url)
            sb = cls(transport, name=name, _ephemeral=False, _telemetry_enabled=telemetry_enabled)
            await sb._connect()
            _record_sandbox_create(sb, image=None, local=local, ephemeral=False, t_start=_t_start)
            return sb

        if image and not runtime and local:
            # local=True with no runtime → auto-select based on image type
            runtime = _auto_runtime(image)
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
                sb = cls(
                    transport, name=name, _ephemeral=ephemeral, _telemetry_enabled=telemetry_enabled
                )
                await sb._connect()
                _record_sandbox_create(
                    sb, image=image, local=False, ephemeral=bool(ephemeral), t_start=_t_start
                )
                return sb
            runtime = _auto_runtime(image)
        if image and runtime:
            sb_name = name or _random_name()
            rt_info = await runtime.start(image, sb_name)
            if rt_info.environment == "android" and not rt_info.qmp_port:
                if rt_info.grpc_port:
                    from cua_sandbox.transport.grpc_emulator import (
                        GRPCEmulatorTransport,
                    )

                    adb_serial = f"emulator-{rt_info.api_port - 1}"
                    sdk_root = None
                    if hasattr(runtime, "_sdk") and runtime._sdk:
                        sdk_root = str(runtime._sdk)
                    transport = GRPCEmulatorTransport(
                        host=rt_info.host,
                        grpc_port=rt_info.grpc_port,
                        serial=adb_serial,
                        sdk_root=sdk_root,
                    )
                else:
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
            elif rt_info.vnc_port and not rt_info.qmp_port and not rt_info.api_port:
                # VNC-only transport: QEMU VMs without a computer-server HTTP API.
                # When api_port is also set (e.g. Docker containers, Lume VMs), prefer HTTP.
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
        # Write persistent state for local (non-ephemeral) sandboxes
        if not ephemeral and rt_info and local:
            from cua_sandbox import sandbox_state

            runtime_type = type(runtime).__name__.lower().replace("runtime", "")
            # Normalize to known types
            _rt_map = {
                "lume": "lume",
                "docker": "docker",
                "qemudocker": "qemu-docker",
                "qemubaremetal": "qemu-baremetal",
                "qemuwsl2": "qemu-wsl2",
            }
            rt_key = _rt_map.get(runtime_type, runtime_type)
            _adb_serial = None
            _sdk_root = None
            if image.os_type == "android":
                _adb_serial = f"emulator-{rt_info.api_port - 1}"
                if hasattr(runtime, "_sdk") and runtime._sdk:
                    _sdk_root = str(runtime._sdk)
            sandbox_state.save(
                sb_name,
                runtime_type=rt_key,
                image=image.to_dict(),
                host=rt_info.host,
                api_port=rt_info.api_port,
                vnc_port=rt_info.vnc_port,
                qmp_port=rt_info.qmp_port,
                grpc_port=rt_info.grpc_port if hasattr(rt_info, "grpc_port") else None,
                adb_serial=_adb_serial,
                sdk_root=_sdk_root,
                os_type=image.os_type,
                status="running",
            )

        resolved_name = (rt_info.name if rt_info else None) or name
        sb = cls(
            transport,
            name=resolved_name,
            _runtime=runtime,
            _runtime_info=rt_info,
            _ephemeral=ephemeral,
            _telemetry_enabled=telemetry_enabled,
        )
        await sb._connect()
        _record_sandbox_create(
            sb, image=image, local=local, ephemeral=bool(ephemeral), t_start=_t_start
        )
        return sb

    def __repr__(self) -> str:
        tname = type(self._transport).__name__
        return f"Sandbox(name={self.name!r}, transport={tname})"


_ADJECTIVES = [
    "amber",
    "bold",
    "calm",
    "deft",
    "eager",
    "fast",
    "glad",
    "hazy",
    "idle",
    "jade",
    "keen",
    "lazy",
    "mild",
    "neat",
    "odd",
    "pale",
    "quiet",
    "rapid",
    "soft",
    "tidy",
    "vast",
    "warm",
    "zany",
    "agile",
    "brave",
    "crisp",
    "dusty",
    "elfin",
    "fizzy",
    "grim",
    "hardy",
    "icy",
    "jolly",
    "kinky",
    "lofty",
    "misty",
    "noble",
    "oaken",
    "prim",
    "quirky",
    "rosy",
    "stark",
    "trim",
    "umber",
    "vivid",
    "witty",
    "xenial",
    "young",
    "zippy",
    "arcane",
    "brisk",
    "chilly",
    "dim",
    "eerie",
    "fleet",
    "gnarly",
    "hushed",
    "inky",
    "jumpy",
    "knotty",
    "lithe",
    "murky",
    "nifty",
    "ornate",
    "plush",
    "quaint",
    "ruddy",
    "spry",
    "tacit",
    "ultra",
    "vague",
    "wily",
    "exact",
    "yare",
    "zesty",
    "arid",
    "blunt",
    "cobalt",
    "dense",
    "ember",
    "faint",
    "gaunt",
    "hollow",
    "irked",
    "jaded",
    "lunar",
    "muted",
    "nimble",
    "opaque",
    "prime",
    "quiet",
    "ringed",
    "sable",
    "tawny",
    "upset",
    "vexed",
    "wooly",
    "xenon",
    "yonder",
    "zingy",
]
_NOUNS = [
    "bear",
    "crane",
    "deer",
    "eagle",
    "finch",
    "gecko",
    "hawk",
    "ibis",
    "jay",
    "kite",
    "lark",
    "mink",
    "newt",
    "orca",
    "puma",
    "quail",
    "raven",
    "seal",
    "toad",
    "vole",
    "wren",
    "yak",
    "zebra",
    "ant",
    "bison",
    "carp",
    "dingo",
    "elk",
    "fox",
    "gull",
    "heron",
    "iguana",
    "jackal",
    "kudu",
    "lemur",
    "moose",
    "narwhal",
    "ocelot",
    "parrot",
    "quokka",
    "rhino",
    "swan",
    "tapir",
    "urial",
    "viper",
    "walrus",
    "xerus",
    "yabby",
    "zorilla",
    "alpaca",
    "beetle",
    "cobra",
    "dugong",
    "emu",
    "ferret",
    "gibbon",
    "hyena",
    "impala",
    "junco",
    "kakapo",
    "lynx",
    "marmot",
    "numbat",
    "osprey",
    "possum",
    "quetzal",
    "rabbit",
    "skunk",
    "thrush",
    "urubu",
    "vulture",
    "wombat",
    "xenops",
    "yaffle",
    "zonkey",
    "addax",
    "booby",
    "condor",
    "dhole",
    "egret",
    "fossa",
    "gannet",
    "hoopoe",
    "indri",
    "jabiru",
    "kookaburra",
    "loris",
    "magpie",
    "nene",
    "olm",
    "pipit",
    "quagga",
    "roller",
    "shrew",
    "teal",
    "uakari",
    "vervet",
    "weevil",
    "xeme",
    "yellowjacket",
    "zorach",
]


def _random_name() -> str:
    return f"{random.choice(_ADJECTIVES)}-{random.choice(_NOUNS)}"


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

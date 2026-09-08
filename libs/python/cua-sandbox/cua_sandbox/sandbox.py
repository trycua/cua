"""Sandbox class — the primary entry point for sandboxed environments.

Exposes .mouse, .keyboard, .screen, .clipboard, .shell, .window, .terminal
as interface objects backed by a Transport.

Usage::

    from cua_sandbox import Image, Pool, Sandbox

    # Provision a persistent sandbox from an explicitly named pool
    # (pool names are globally unique across accounts)
    pool = await Pool.apply(Image.desktop("ubuntu"), name="my-pool")
    sb = await Sandbox.create(pool=pool, name="my-claim")
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

import logging
import random
import secrets
import time
from collections.abc import Mapping
from contextlib import asynccontextmanager
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
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
    from cua_core.telemetry import is_telemetry_enabled, record_event

    _TELEMETRY_AVAILABLE = True
except ImportError:
    _TELEMETRY_AVAILABLE = False

    def is_telemetry_enabled() -> bool:
        return False

    def record_event(event_name: str, properties: dict | None = None) -> None:
        pass


from cua_sandbox._config import has_fleet_auth
from cua_sandbox.image import Image
from cua_sandbox.interfaces import (
    Apps,
    Clipboard,
    Files,
    Keyboard,
    Mobile,
    Mouse,
    Screen,
    Services,
    Shell,
    Terminal,
    Tunnel,
    Window,
)
from cua_sandbox.transport.base import Transport
from cua_sandbox.transport.cloud import CloudTransport
from cua_sandbox.transport.fleet_cloud import FleetCloudTransport
from cua_sandbox.transport.http import HTTPTransport
from cua_sandbox.transport.websocket import WebSocketTransport

if TYPE_CHECKING:
    from cua_sandbox.pool import Pool
    from cua_sandbox.runtime.base import Runtime, RuntimeInfo

logger = logging.getLogger(__name__)

_T = TypeVar("_T")


async def _keep_alive_or_close(sandbox: Any, minutes: float | None) -> None:
    if minutes is None:
        return
    try:
        await sandbox.keep_alive(minutes=minutes)
    except BaseException as keep_alive_error:
        try:
            await sandbox.close()
        except BaseException as close_error:
            logger.warning("Failed to close Fleet claim after keep-alive failure: %s", close_error)
            raise keep_alive_error from close_error
        raise


async def _save_fleet_claim_or_close(
    sandbox: Any,
    claim_name: str,
    pool_name: str,
) -> None:
    from cua_sandbox import sandbox_state

    try:
        sandbox_state.save_fleet_claim(claim_name, pool_name)
    except BaseException as state_error:
        try:
            await sandbox.close()
        except BaseException as close_error:
            raise state_error from close_error
        raise


async def _cleanup_ephemeral_fleet(
    sandbox: Any | None,
    pool: Any | None,
    *,
    suppress_errors: bool,
) -> None:
    cleanup_error: BaseException | None = None
    cleanup_operations = (
        ("claim", sandbox.close if sandbox is not None else None),
        ("pool", pool.delete if pool is not None else None),
    )
    for resource, cleanup in cleanup_operations:
        if cleanup is None:
            continue
        try:
            await cleanup()
        except BaseException as error:
            if suppress_errors or cleanup_error is not None:
                logger.warning("Failed to clean up ephemeral Fleet %s: %s", resource, error)
            else:
                cleanup_error = error
    if cleanup_error is not None:
        raise cleanup_error


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


def _remove_orphan_container(name: str) -> bool:
    """Remove a container left behind by a launch that never wrote state.

    A local launch that times out during the readiness probe leaves a running
    container and no state file, which put it beyond the reach of
    ``Sandbox.delete``. Returns whether a container was actually removed.
    """
    import subprocess

    try:
        exists = subprocess.run(
            ["docker", "inspect", "--type", "container", name],
            capture_output=True,
        )
        if exists.returncode != 0:
            return False
        removed = subprocess.run(["docker", "rm", "-f", name], capture_output=True)
        return removed.returncode == 0
    except (OSError, subprocess.SubprocessError):
        # Docker missing or unusable — no orphan we can claim to have removed.
        return False


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

    if image.os_type == "linux":
        # A Linux VM boots the pinned containerDisk under bare-metal QEMU — the
        # same disk Fleet cloud boots. Docker-wrapped QEMU cannot reach that path
        # at all: resolve_image() hands it the XFCE *container* image, so asking
        # for a VM used to quietly get you a container.
        from cua_sandbox.runtime.compat import _has_qemu

        if not _has_qemu():
            raise RuntimeError(
                "Image.linux() is a VM and needs QEMU, which was not found on "
                "this host. Install it:\n"
                "  Debian/Ubuntu:  sudo apt install qemu-system-x86\n"
                "  Fedora/RHEL:    sudo dnf install qemu-system-x86\n"
                "  macOS:          brew install qemu\n"
                "Or pass an explicit runtime= if you want a different one."
            )
        return QEMURuntime(mode="bare-metal")

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
        self._claim_handle: Any = None
        self._claim_released = False
        self.telemetry_enabled = _telemetry_enabled
        self.screen = Screen(transport)
        self.mouse = Mouse(transport)
        self.keyboard = Keyboard(transport)
        self.clipboard = Clipboard(transport)
        self.shell = Shell(transport)
        self.files = Files(transport)
        self.window = Window(transport)
        self.terminal = Terminal(transport)
        self.mobile = Mobile(transport)
        self.tunnel = Tunnel(transport)
        self.services = Services(transport)
        _os = _runtime_info.environment if _runtime_info and _runtime_info.environment else "linux"
        self.apps = Apps(transport, os_type=_os)

    async def _connect(self) -> None:
        await self._transport.connect()
        # Update name from transport (e.g. CloudTransport resolves name after creating a VM)
        if self.name is None and isinstance(self._transport, (CloudTransport, FleetCloudTransport)):
            self.name = self._transport.name

    async def disconnect(self) -> None:
        """Drop the transport connection. The sandbox keeps running."""
        await self._transport.disconnect()

    @property
    def exposed_ports(self) -> dict:
        """Map each Image.expose() port to the host port forwarding it.

        Local sandboxes forward exposed ports to free host ports chosen at boot,
        so the mapping is only knowable at runtime. Reading it from the saved
        state as well means a reconnecting caller can still find the port rather
        than it living only on the object create() returned. Empty when the
        runtime forwards nothing (Fleet publishes services instead — use
        tunnel.forward()).
        """
        info = getattr(self, "_runtime_info", None)
        ports = getattr(info, "exposed_ports", None) if info else None
        if ports:
            return dict(ports)
        name = getattr(self, "name", None)
        if name:
            try:
                from cua_sandbox import sandbox_state

                saved = sandbox_state.load(name) or {}
            except Exception:  # noqa: BLE001 - a missing state file is not an error here
                saved = {}
            stored = saved.get("exposed_ports") or {}
            # JSON object keys are strings; callers index by guest port int.
            return {int(guest): host for guest, host in stored.items()}
        return {}

    @property
    def claim_name(self) -> str | None:
        """Fleet claim name, distinct from the bound sandbox name."""
        return self._claim_handle.name if self._claim_handle is not None else None

    @property
    def pool_name(self) -> str | None:
        """Fleet pool that owns this claim."""
        return self._claim_handle.pool_name if self._claim_handle is not None else None

    def to_dict(self) -> dict[str, Any]:
        """Serialize a durable Fleet sandbox reference."""
        if self._claim_handle is None:
            raise NotImplementedError("serialization is only supported for Fleet claims")
        return self._claim_handle.to_dict()

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "_ConnectResult":
        """Reconnect to a serialized Fleet claim reference."""

        async def factory() -> "Sandbox":
            from cua_sandbox.pool import _ClaimHandle

            handle = _ClaimHandle.from_dict(data)
            if handle.namespace != handle.pool_name:
                raise ValueError("serialized claim does not belong to the requested pool")
            return await handle.wait()

        return _ConnectResult(factory)

    async def keep_alive(self, *, minutes: float) -> None:
        """Push a Fleet claim's controller-enforced shutdown time forward."""
        if self._claim_handle is None:
            raise NotImplementedError("keep_alive is only supported for Fleet claims")
        if minutes <= 0:
            raise ValueError("minutes must be positive")
        shutdown_time = (datetime.now(timezone.utc) + timedelta(minutes=minutes)).isoformat()
        await self._claim_handle.renew(shutdown_time.replace("+00:00", "Z"))

    async def close(self) -> None:
        """Release this Fleet claim; repeated calls are safe."""
        if self._claim_released:
            return
        if self._claim_handle is None:
            raise NotImplementedError("close is only supported for Fleet claims")
        claim_name = self.claim_name
        try:
            await self._claim_handle.release()
            self._claim_released = True
            if claim_name is not None:
                from cua_sandbox import sandbox_state

                try:
                    sandbox_state.delete(claim_name)
                except OSError as error:
                    logger.warning("Failed to remove Fleet claim state %r: %s", claim_name, error)
        finally:
            await self.disconnect()

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

        if not isinstance(self._transport, (CloudTransport, FleetCloudTransport)):
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
            logger.warning(
                "Destroying sandbox %s which has snapshots — "
                "forks referencing those snapshots will break. "
                "Use Sandbox.ephemeral() which auto-stops instead of deleting "
                "when snapshots exist.",
                self.name,
            )
        if self.telemetry_enabled and _TELEMETRY_AVAILABLE and is_telemetry_enabled():
            record_event("sandbox_destroy", {"name": self.name, "ephemeral": self._ephemeral})
        # Run each cleanup step independently so a failure in one
        # (e.g. disconnect timeout) doesn't prevent the VM from being deleted.
        try:
            await self._transport.disconnect()
        except Exception:
            logger.warning("Failed to disconnect transport for sandbox %r", self.name)
        if isinstance(self._transport, (CloudTransport, FleetCloudTransport)):
            try:
                await self._transport.delete_vm()
            except Exception:
                logger.warning("Failed to delete cloud VM %r", self.name)
        if self._runtime and self._runtime_info:
            vm_name = self._runtime_info.name or self.name or "cua-sandbox"
            try:
                if self._ephemeral and hasattr(self._runtime, "delete"):
                    await self._runtime.delete(vm_name)
                else:
                    await self._runtime.stop(vm_name)
            except Exception:
                logger.warning("Failed to stop/delete runtime for sandbox %r", self.name)

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
        image: Image | None = None,
        *,
        pool: "Pool | str | None" = None,
        name: Optional[str] = None,
        replicas: int = 1,
        service: str = "server",
        claim_spec: Any = None,
        keep_alive_minutes: float | None = None,
        api_key: Optional[str] = None,
        local: bool = False,
        runtime: Optional["Runtime"] = None,
        cpu: Optional[int] = None,
        memory_mb: Optional[int] = None,
        disk_gb: Optional[int] = None,
        region: str = "us-east-1",
        time_to_start: Optional[float] = None,
        request_timeout: Optional[float] = None,
        server_port: int = 8000,
        telemetry_enabled: bool = True,
    ) -> "Sandbox":
        """Provision or claim a persistent sandbox and return it connected.

        Supplying ``pool`` claims from an existing Fleet pool without changing
        its configuration. Fleet registry images require an explicitly named
        pool — apply one with ``Pool.apply(image, name=...)`` and pass it as
        ``pool=`` — because pool names are globally unique across accounts.
        """
        from cua_sandbox.pool import Pool

        if (
            isinstance(server_port, bool)
            or not isinstance(server_port, int)
            or server_port < 1
            or server_port > 65535
        ):
            raise ValueError("server_port must be an integer between 1 and 65535")

        if pool is not None:
            if image is not None:
                raise ValueError("image and pool are mutually exclusive")
            if replicas != 1 or cpu is not None or memory_mb is not None or disk_gb is not None:
                raise ValueError("configuration cannot be supplied for an existing pool")
            if (
                local
                or runtime is not None
                or api_key is not None
                or region != "us-east-1"
                or request_timeout is not None
                or server_port != 8000
            ):
                raise NotImplementedError("the requested option is not supported with Fleet pools")
            resolved_pool = await Pool.get(pool) if isinstance(pool, str) else pool
            sandbox = await resolved_pool.claim(
                name=name, spec=claim_spec, service=service, time_to_start=time_to_start
            )
            sandbox_claim_name = getattr(sandbox, "claim_name", None)
            sandbox_pool_name = getattr(sandbox, "pool_name", None)
            claim_name = (
                sandbox_claim_name
                if isinstance(sandbox_claim_name, str) and sandbox_claim_name
                else name
            )
            pool_name = (
                sandbox_pool_name
                if isinstance(sandbox_pool_name, str) and sandbox_pool_name
                else resolved_pool.name
            )
            await _keep_alive_or_close(sandbox, keep_alive_minutes)
            if claim_name is not None:
                await _save_fleet_claim_or_close(sandbox, claim_name, pool_name)
            return sandbox

        from cua_sandbox.image import cloud_registry_image

        fleet_image = (
            image is not None
            and cloud_registry_image(image) is not None
            and cls._uses_fleet(api_key)
            and not local
            and runtime is None
        )
        if fleet_image:
            raise ValueError(
                "Sandbox.create requires an explicitly named pool for Fleet "
                "registry images because pool names are globally unique "
                "across accounts. Apply a pool first, then claim from it:\n"
                '    pool = await Pool.apply(image, name="my-pool")\n'
                '    sandbox = await Sandbox.create(pool=pool, name="my-claim")\n'
                "Or use Sandbox.ephemeral(image, ...) for a disposable sandbox."
            )

        if image is None:
            raise ValueError("image is required when pool is omitted")
        if replicas != 1 or service != "server" or claim_spec is not None:
            raise NotImplementedError("claim options are only supported by Fleet")
        return await cls._create(
            image=image,
            name=name,
            pool=pool,
            ephemeral=False,
            api_key=api_key,
            local=local,
            runtime=runtime,
            cpu=cpu,
            memory_mb=memory_mb,
            disk_gb=disk_gb,
            region=region,
            time_to_start=time_to_start,
            request_timeout=request_timeout,
            server_port=server_port,
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
        image: Image | None = None,
        *,
        pool: "Pool | str | None" = None,
        name: Optional[str] = None,
        replicas: int = 1,
        service: str = "server",
        claim_spec: Any = None,
        keep_alive_minutes: float | None = None,
        keep_pool: bool = False,
        api_key: Optional[str] = None,
        local: bool = False,
        runtime: Optional["Runtime"] = None,
        cpu: Optional[int] = None,
        memory_mb: Optional[int] = None,
        disk_gb: Optional[int] = None,
        region: str = "us-east-1",
        time_to_start: Optional[float] = None,
        request_timeout: Optional[float] = None,
        server_port: int = 8000,
        telemetry_enabled: bool = True,
    ) -> AsyncIterator["Sandbox"]:
        from cua_sandbox.image import cloud_registry_image
        from cua_sandbox.pool import Pool

        fleet_image = (
            image is not None
            and cloud_registry_image(image) is not None
            and cls._uses_fleet(api_key)
            and not local
            and runtime is None
        )
        if keep_pool and not fleet_image:
            raise ValueError("keep_pool is only supported for Fleet registry images")
        if keep_pool and name is None:
            raise ValueError(
                "keep_pool requires name= so later runs can find and reuse the "
                "kept pool; pool names are globally unique across accounts"
            )

        if pool is not None:
            sandbox = await cls.create(
                image,
                pool=pool,
                name=name,
                replicas=replicas,
                service=service,
                claim_spec=claim_spec,
                keep_alive_minutes=keep_alive_minutes,
                api_key=api_key,
                local=local,
                runtime=runtime,
                cpu=cpu,
                memory_mb=memory_mb,
                disk_gb=disk_gb,
                region=region,
                time_to_start=time_to_start,
                request_timeout=request_timeout,
                server_port=server_port,
                telemetry_enabled=telemetry_enabled,
            )
            try:
                yield sandbox
            except BaseException:
                await _cleanup_ephemeral_fleet(sandbox, None, suppress_errors=True)
                raise
            else:
                await _cleanup_ephemeral_fleet(sandbox, None, suppress_errors=False)
            return

        if fleet_image:
            if disk_gb is not None or region != "us-east-1" or request_timeout is not None:
                raise NotImplementedError("the requested option is not supported by Fleet")
            if (
                isinstance(server_port, bool)
                or not isinstance(server_port, int)
                or server_port < 1
                or server_port > 65535
            ):
                raise ValueError("server_port must be an integer between 1 and 65535")
            services = {
                "server": server_port,
                **{f"port-{port}": port for port in image._ports if port != server_port},
            }
            # A random pool name keeps this disposable pool out of foreign
            # namespaces: pool names are globally unique across accounts.
            pool_name = name if name is not None else f"cua-eph-{secrets.token_hex(6)}"
            owned_pool = await Pool.apply(
                image,
                name=pool_name,
                replicas=replicas,
                cpu=cpu,
                memory_mb=memory_mb,
                services=services,
            )
            sandbox = None
            try:
                sandbox = await owned_pool.claim(
                    name=name,
                    spec=claim_spec,
                    service=service,
                    time_to_start=time_to_start,
                )
                await _keep_alive_or_close(sandbox, keep_alive_minutes)
            except BaseException:
                await _cleanup_ephemeral_fleet(
                    None,
                    None if keep_pool else owned_pool,
                    suppress_errors=True,
                )
                raise

            try:
                yield sandbox
            except BaseException:
                await _cleanup_ephemeral_fleet(
                    sandbox,
                    None if keep_pool else owned_pool,
                    suppress_errors=True,
                )
                raise
            else:
                await _cleanup_ephemeral_fleet(
                    sandbox,
                    None if keep_pool else owned_pool,
                    suppress_errors=False,
                )
            return

        if image is None:
            raise ValueError("image is required when pool is omitted")
        sandbox = await cls._create(
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
            time_to_start=time_to_start,
            request_timeout=request_timeout,
            server_port=server_port,
            telemetry_enabled=telemetry_enabled,
        )
        try:
            yield sandbox
        finally:
            if sandbox._has_snapshots and sandbox.name:
                await cls.suspend(sandbox.name, local=local, api_key=api_key)
            else:
                await sandbox.destroy()

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

    @staticmethod
    def _uses_fleet(api_key: Optional[str]) -> bool:
        """Choose Fleet only for OAuth-configured calls without an explicit API key."""
        return api_key is None and has_fleet_auth()

    @classmethod
    async def _list_cloud(cls, *, api_key: Optional[str] = None) -> "list[SandboxInfo]":
        if not cls._uses_fleet(api_key):
            from cua_sandbox.transport.cloud import cloud_list_vms

            vms = await cloud_list_vms(api_key=api_key)
            return [
                SandboxInfo(
                    name=vm.get("name", ""),
                    status=vm.get("status", "unknown"),
                    source="cloud",
                    os_type=vm.get("os_type") or vm.get("os"),
                    created_at=vm.get("created_at"),
                )
                for vm in vms
            ]

        pools = await FleetCloudTransport.list_sandboxes()
        return [cls._fleet_sandbox_info(pool) for pool in pools]

    @staticmethod
    def _fleet_sandbox_info(pool: Any) -> SandboxInfo:
        if isinstance(pool, Mapping):
            metadata = pool.get("metadata") or {}
            spec = pool.get("spec") or {}
            status = pool.get("status") or {}
            name = metadata.get("name", "")
            replicas = spec.get("replicas", 1)
            ready = status.get("readyReplicas", 0)
            created_at = metadata.get("creationTimestamp")
        else:
            metadata = pool.metadata
            spec = pool.spec
            status = pool.status
            name = metadata.name
            replicas = spec.replicas
            ready = status.ready_replicas if status else 0
            created_at = metadata.creation_timestamp
        state = "suspended" if replicas == 0 else "running" if ready else "provisioning"
        return SandboxInfo(
            name=name,
            status=state,
            source="fleet",
            created_at=created_at,
        )

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
        if not cls._uses_fleet(api_key):
            from cua_sandbox.transport.cloud import cloud_get_vm

            vm = await cloud_get_vm(name, api_key=api_key)
            return SandboxInfo(
                name=vm.get("name", name),
                status=vm.get("status", "unknown"),
                source="cloud",
                os_type=vm.get("os_type") or vm.get("os"),
                created_at=vm.get("created_at"),
            )
        return cls._fleet_sandbox_info(await FleetCloudTransport.get_sandbox_info(name))

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
        if not cls._uses_fleet(api_key):
            from cua_sandbox.transport.cloud import cloud_vm_action

            await cloud_vm_action(name, "stop", api_key=api_key)
            return
        await FleetCloudTransport.suspend_sandbox(name)

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
        if not cls._uses_fleet(api_key):
            from cua_sandbox.transport.cloud import cloud_vm_action

            await cloud_vm_action(name, "run", api_key=api_key)
        else:
            await FleetCloudTransport.resume_sandbox(name)
        # Connect to the now-running cloud sandbox.
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
            from cua_sandbox.runtime.lume import LumeRuntime

            image = Image.from_dict(state["image"])
            rt = LumeRuntime()
            rt_info = await rt.resume(image, name)
        elif runtime_type == "qemu-baremetal":
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
        if not cls._uses_fleet(api_key):
            from cua_sandbox.transport.cloud import cloud_vm_action

            await cloud_vm_action(name, "restart", api_key=api_key)
        else:
            await FleetCloudTransport.restart_sandbox(name)
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
        if not cls._uses_fleet(api_key):
            from cua_sandbox.transport.cloud import cloud_vm_action

            await cloud_vm_action(name, "delete", api_key=api_key)
            return
        from cua_sandbox import sandbox_state

        state = sandbox_state.load(name)
        pool_name = state.get("pool_name") if state else None
        await FleetCloudTransport.delete_sandbox(name, pool_name=pool_name)
        if pool_name:
            sandbox_state.delete(name)

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
        elif not _remove_orphan_container(name):
            # No state file and no container by that name — deleting nothing at
            # all used to report success, so a typo looked like a deletion.
            raise ValueError(f"No local sandbox named {name!r}")
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
        pool: Optional[str] = None,
        ephemeral: Optional[bool] = None,
        cpu: Optional[int] = None,
        memory_mb: Optional[int] = None,
        disk_gb: Optional[int] = None,
        region: str = "us-east-1",
        time_to_start: Optional[float] = None,
        request_timeout: Optional[float] = None,
        server_port: int = 8000,
        telemetry_enabled: bool = True,
    ) -> "Sandbox":
        """Internal factory that validates server_port before selecting a transport."""
        if (
            isinstance(server_port, bool)
            or not isinstance(server_port, int)
            or server_port < 1
            or server_port > 65535
        ):
            raise ValueError("server_port must be an integer between 1 and 65535")

        if image is not None and pool is not None:
            raise ValueError("Specify exactly one of image or pool")
        if pool and not name:
            raise ValueError("Pool-backed sandboxes require a name")
        if pool and local:
            raise ValueError("Pool-backed sandboxes are cloud-only")

        _t_start = time.monotonic()
        if ephemeral is None:
            ephemeral = bool(image)

        rt_info = None
        if image and image.kind is None and image._registry and local:
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

        if pool:
            transport = FleetCloudTransport(
                image=None,
                name=name,
                pool_name=pool,
                create_claim=True,
                region=region,
                time_to_start=time_to_start,
                request_timeout=request_timeout,
                server_port=server_port,
            )
            sb = cls(transport, name=name, _ephemeral=False, _telemetry_enabled=telemetry_enabled)
            await sb._connect()
            from cua_sandbox import sandbox_state

            sandbox_state.save_fleet_claim(name, pool)
            _record_sandbox_create(sb, image=None, local=False, ephemeral=False, t_start=_t_start)
            return sb

        if image and not runtime and local:
            # local=True with no runtime → auto-select based on image type
            runtime = _auto_runtime(image)
        if image and not runtime and not local:
            # image without runtime and not local → cloud creation
            if not any([ws_url, http_url]) and cls._uses_fleet(api_key):
                transport = FleetCloudTransport(
                    image=image,
                    name=name or _random_name(),
                    cpu=cpu,
                    memory_mb=memory_mb,
                    disk_gb=disk_gb,
                    region=region,
                    time_to_start=time_to_start,
                    request_timeout=request_timeout,
                    server_port=server_port,
                )
                sb = cls(
                    transport, name=name, _ephemeral=ephemeral, _telemetry_enabled=telemetry_enabled
                )
                try:
                    await sb._connect()
                except BaseException:
                    # _connect() calls CloudTransport.connect() which may have
                    # already created a VM before failing (e.g. timeout while
                    # polling for "running" status).  Delete the orphan so it
                    # doesn't leak.
                    vm_name = transport._name
                    if vm_name:
                        try:
                            await transport.delete_vm()
                        except Exception:
                            logger.warning(
                                "Failed to clean up cloud VM %r after connect failure",
                                vm_name,
                            )
                    raise
                _record_sandbox_create(
                    sb, image=image, local=False, ephemeral=bool(ephemeral), t_start=_t_start
                )
                return sb
            if not any([ws_url, http_url]):
                transport = _make_transport(
                    api_key=api_key,
                    image=image,
                    name=name,
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
            rt_info = await runtime.start(image, sb_name, ephemeral=bool(ephemeral))
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
            if name and cls._uses_fleet(api_key) and not ws_url and not http_url:
                from cua_sandbox import sandbox_state

                state = sandbox_state.load(name)
                pool_name = state.get("pool_name") if state else None
                transport = FleetCloudTransport(
                    image=None,
                    name=name,
                    pool_name=pool_name,
                    cpu=cpu,
                    memory_mb=memory_mb,
                    disk_gb=disk_gb,
                    region=region,
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
    image: Optional[Image] = None,
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
        image=image,
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

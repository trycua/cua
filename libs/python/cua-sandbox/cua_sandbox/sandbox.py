"""Sandbox class — the primary entry point for sandboxed environments.

Exposes .mouse, .keyboard, .screen, .clipboard, .shell, .window, .terminal
as interface objects backed by a Transport.

cua-sandbox is a thin wrapper over the ``cua`` SDK: Fleet claims, the
Docker/QEMU/Lume runtimes and cua-spacesd (the interfaces) all go through
it. Sandboxes are daemon-agnostic: creating, connecting and deleting one never
assumes a guest agent; the interfaces use cua-spacesd when the image has it
and an agentless path (QMP, VNC, SSH, ADB) otherwise.

Usage::

    from cua_sandbox import Image, Sandbox, http

    # Any image, local or cloud: the same code, flip on= (or local=).
    # Unset, `cua config set default.on cloud` / CUA_DEFAULT_ON decide.
    sb = await Sandbox.create(
        Image.from_registry("python:3.12-slim"),
        command=["python", "-m", "my_mcp", "--port", "8765"],
        services={"mcp": 8765},
        wait_for=http("mcp", "/health"),
        on="local",          # "local" | "cloud"
        kind="container",    # "auto" | "container" | "vm"
        runtime="gvisor",    # the engine: "auto", gvisor/runc/qemu/lume, kubevirt
    )
    r = await sb.service("mcp").request("POST", "/mcp", json={...}, headers={...})
    url = await sb.public_url("mcp", ttl=3600)
    await sb.destroy()

    # Connect to an existing sandbox by name (plain await or async with)
    sb = await Sandbox.connect("my-sandbox")

    # ...or to any reachable cua-spacesd by URL
    sb = await Sandbox.connect(url="http://10.0.0.5:3211", token="...")
    await sb.screenshot()
    await sb.disconnect()

    async with Sandbox.connect("my-sandbox") as sb:  # disconnects on exit
        await sb.screenshot()

    # Ephemeral — auto-destroyed on exit
    async with Sandbox.ephemeral(Image.linux()) as sb:
        await sb.shell.run("whoami")
"""

from __future__ import annotations

import asyncio
import logging
import os
import random
import time
import warnings
from collections.abc import Mapping
from contextlib import asynccontextmanager
from dataclasses import dataclass, field
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


from cua_sandbox import _placement, _refs
from cua_sandbox._config import has_fleet_access
from cua_sandbox._refs import AmbiguousSandbox  # noqa: F401 - re-exported
from cua_sandbox._sdk import (  # noqa: F401 - InvalidPlacement is re-exported
    ENV_SERVICE,
    InvalidArgument,
    InvalidPlacement,
    SpacesdNotAvailable,
    Unsupported,
)
from cua_sandbox.image import Image, ImageInfo
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
from cua_sandbox.interfaces import mcp as _mcp
from cua_sandbox.interfaces.driver import Driver
from cua_sandbox.options import (
    CloudOptions,
    Probe,
    PublicUrl,
    WaitFor,
    check_network,
    check_services,
    parse_memory,
    probes,
)
from cua_sandbox.spec import SandboxSpec
from cua_sandbox.transport.base import Transport
from cua_sandbox.transport.cloud import CloudTransport
from cua_sandbox.transport.env import EnvTransport, env_url
from cua_sandbox.transport.fleet_cloud import (
    FleetCloudTransport,
    OSWorldFleetCloudTransport,
    validate_server_port,
)
from cua_sandbox.transport.osworld import OSWORLD_SERVER_PORT

if TYPE_CHECKING:
    from cua_sandbox.interfaces.services import ServiceHandle
    from cua_sandbox.pool import Pool
    from cua_sandbox.runtime.base import Runtime, RuntimeInfo
    from cua_sandbox.transport.fleet_cloud import FleetRuntime

logger = logging.getLogger(__name__)

_T = TypeVar("_T")


def _overlay_specs(overlay: Any) -> list:
    """``{name: path}`` / ``{name: (path, guest_path)}`` / a list of
    ``cua.Overlay`` into native overlays."""
    if not overlay:
        return []
    if isinstance(overlay, dict):
        from cua_sandbox._sdk import sdk

        native = sdk()._native
        out = []
        for name, spec in overlay.items():
            path, target = (spec, None) if isinstance(spec, (str, os.PathLike)) else spec
            out.append(
                native.Overlay(name=str(name), path=os.fspath(path), target=target, source=None)
            )
        return out
    return list(overlay)


async def _with_overlays(sandbox: "Sandbox", overlay: Any) -> "Sandbox":
    """Applies ``overlay`` to a new sandbox; deletes it if that fails (a
    sandbox that cannot run the build under test is not returned)."""
    if not overlay:
        return sandbox
    try:
        await sandbox.overlay(overlay)
    except BaseException:
        try:
            await sandbox.destroy()
        except Exception:  # noqa: BLE001 - the overlay error is the one to raise
            logger.warning("could not delete sandbox %r after a failed overlay", sandbox.name)
        raise
    return sandbox


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
    **extra: Any,
) -> None:
    from cua_sandbox import sandbox_state

    try:
        sandbox_state.save_fleet_claim(claim_name, pool_name, **extra)
    except BaseException as state_error:
        try:
            await sandbox.close()
        except BaseException as close_error:
            raise state_error from close_error
        raise


def _reject_managed_options_for_pool(
    warm: Optional[bool], max_pool_size: Optional[int], claim_ttl: Any
) -> None:
    if warm is not None or max_pool_size is not None or claim_ttl is not None:
        raise ValueError(
            "warm, max_pool_size and claim_ttl configure managed pools (image "
            "without pool=). An explicit pool keeps its own configuration: size it "
            "with Pool.apply(..., autoscaling=...) and set a claim TTL with "
            "claim_spec= or pool.claim(ttl_seconds_after_created=...)."
        )


def _check_managed_options(
    *,
    replicas: int,
    claim_spec: Any,
    disk_gb: Optional[int],
    region: str,
    request_timeout: Optional[float],
) -> None:
    if replicas != 1:
        raise ValueError(
            "replicas= sizes an explicit pool. Managed pools autoscale from zero: "
            "use warm=True and max_pool_size= instead, or Pool.apply(image, "
            "name=..., replicas=...) and pass it as pool="
        )
    if claim_spec is not None:
        raise ValueError(
            "claim_spec= needs an explicit pool=. Managed pools set the claim "
            "TTL from claim_ttl= and renew it while the sandbox is held"
        )
    if disk_gb is not None or region != "us-east-1" or request_timeout is not None:
        raise NotImplementedError("the requested option is not supported by Fleet")


def _is_local_sandbox(name: str) -> bool:
    """Whether ``name`` is a local sandbox (a state file that is not a Fleet
    claim)."""
    from cua_sandbox import sandbox_state

    state = sandbox_state.load(name)
    return bool(state) and state.get("runtime_type") != "fleet"


async def _lookup(name: str, local: Optional[bool]) -> tuple[str, bool]:
    """``(plain name, local)`` for a sandbox ref or bare name (see
    :func:`cua_sandbox._refs.resolve`). A ``direct:`` machine is not managed
    here: connect to it with ``Sandbox.connect(url=...)``."""
    plain, is_local, direct = await _refs.resolve(name, local)
    if direct is not None:
        raise Unsupported(
            f"{name}: a direct machine is not managed by cua-sandbox; connect to it with "
            f"Sandbox.connect({name!r})"
        )
    return plain, bool(is_local)


def _fleet_lifecycle_unsupported(name: str, operation: str) -> Unsupported:
    """Fleet has no per-sandbox suspend: a claim is running or released, and
    scaling its pool would touch every sandbox in it."""
    return Unsupported(
        f"{operation} is not supported for cloud sandboxes ({name!r}): Fleet cannot "
        "suspend a single sandbox. Hold it with keep_alive(), or release it with "
        "Sandbox.delete(name) and create a new one"
    )


async def _acquire_managed(
    image: Image,
    *,
    name: Optional[str],
    service: str,
    cpu: Optional[int],
    memory_mb: Optional[int],
    server_port: Optional[int],
    time_to_start: Optional[float],
    warm: Optional[bool],
    max_pool_size: Optional[int],
    claim_ttl: Any,
    progress: Optional[Callable[[Any], Any]],
    telemetry_enabled: bool,
    ephemeral: bool = False,
    fleet_runtime: Optional[str] = None,
    command: Optional[list[str]] = None,
    sidecars: Optional[list] = None,
    env: Optional[dict[str, str]] = None,
    services: Optional[dict[str, int]] = None,
    wait_for: Optional[list[Probe]] = None,
    hint: Optional[str] = None,
) -> "Sandbox":
    """Claim ``image`` from this account's managed cloud capacity. ``hint``
    (the cloud came from a user default) is added to a missing-credentials
    error."""
    from cua_sandbox import _autopool

    validate_server_port(server_port)
    t_start = time.monotonic()
    sandbox = await _autopool.acquire(
        image,
        name=name,
        cpu=cpu,
        memory_mb=memory_mb,
        server_port=server_port,
        service=service,
        time_to_start=time_to_start,
        cfg=_autopool.config(warm=warm, max_pool_size=max_pool_size, claim_ttl=claim_ttl),
        progress=progress,
        runtime=fleet_runtime,
        command=command,
        sidecars=sidecars,
        env=env,
        services=services,
        wait_for=wait_for,
        hint=hint,
    )
    sandbox._ephemeral = ephemeral
    sandbox.telemetry_enabled = telemetry_enabled
    _record_sandbox_create(sandbox, image=image, local=False, ephemeral=ephemeral, t_start=t_start)
    return sandbox


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
    """Metadata for a local or cloud sandbox.

    ``status`` from :meth:`Sandbox.info` is portable: ``provisioning``,
    ``starting``, ``ready`` or ``stopped``. Listings keep the provider's word
    (``running``, ``suspended``, ...). Provider internals (the cloud pool,
    namespace and claim; the local backend) are in ``provider_details``.
    """

    name: str
    status: str
    source: str  # "cloud" | "fleet" | "lume" | "docker" | "qemu-baremetal" | ...
    os_type: Optional[str] = None
    host: Optional[str] = None
    vnc_url: Optional[str] = None
    api_url: Optional[str] = None
    created_at: Optional[str] = None
    #: The qualified ref: ``local:<name>``, ``cloud:<name>`` or
    #: ``direct:<host:port>`` (what ``Sandbox.connect`` / ``delete`` take).
    id: Optional[str] = None
    location: Optional[str] = None  # "local" | "cloud" | "direct"
    services: dict = field(default_factory=dict)
    expires_at: Optional[str] = None
    provider_details: dict = field(default_factory=dict)
    #: What kind of machine: ``container`` or ``vm`` (``None`` when unknown).
    kind: Optional[str] = None
    #: Which engine runs it: ``gvisor``, ``runc``, ``qemu``, ``lume`` (local),
    #: ``gvisor``, ``kubevirt`` (cloud); ``None`` when unknown.
    runtime: Optional[str] = None

    def __post_init__(self) -> None:
        # The qualified ref (`local:<name>`, `cloud:<name>`) when not given.
        if self.id is None and self.location and self.name:
            self.id = f"{self.location}:{self.name}"

    def __getattr__(self, name: str) -> Any:
        if name == "pool":
            warnings.warn(
                "SandboxInfo.pool is deprecated; use provider_details.get('pool')",
                DeprecationWarning,
                stacklevel=2,
            )
            return self.__dict__.get("provider_details", {}).get("pool")
        raise AttributeError(name)


_PHASE_WORDS = {
    "PROVISIONING": "provisioning",
    "STARTING": "starting",
    "READY": "ready",
    "STOPPED": "stopped",
}


def _merge_cloud(
    cloud: Optional[CloudOptions],
    *,
    pool: Any,
    warm: Optional[bool],
    max_pool_size: Optional[int],
    claim_ttl: Any,
) -> tuple[Any, Optional[bool], Optional[int], Any]:
    """``cloud=`` merged with the deprecated flat kwargs (pool, warm,
    max_pool_size, claim_ttl)."""
    old = {
        "pool": pool,
        "warm": warm,
        "max_pool_size": max_pool_size,
        "claim_ttl": claim_ttl,
    }
    given = [k for k, v in old.items() if v is not None]
    if given:
        warnings.warn(
            f"{', '.join(given)}= {'is' if len(given) == 1 else 'are'} deprecated; pass "
            f"cloud=CloudOptions({', '.join(f'{k}=...' for k in given)})",
            DeprecationWarning,
            stacklevel=4,
        )
    if cloud is None:
        return pool, warm, max_pool_size, claim_ttl
    if not isinstance(cloud, CloudOptions):
        raise TypeError("cloud= takes a CloudOptions")
    merged = []
    for key in ("pool", "warm", "max_pool_size", "claim_ttl"):
        new, previous = getattr(cloud, key), old[key]
        if new is not None and previous is not None and new != previous:
            raise ValueError(f"{key} is set both in cloud= and as a keyword argument")
        merged.append(new if new is not None else previous)
    return tuple(merged)  # type: ignore[return-value]


#: How long the default ``Sandbox.list()`` waits for the cloud.
_LIST_CLOUD_TIMEOUT = 5.0


def _cloud_only_args(
    *,
    pool: Any = None,
    warm: Any = None,
    max_pool_size: Any = None,
    claim_ttl: Any = None,
    claim_spec: Any = None,
    api_key: Any = None,
    replicas: int = 1,
    service: str = ENV_SERVICE,
    region: str = "us-east-1",
    keep_alive_minutes: Any = None,
    progress: Any = None,
) -> list[str]:
    """The keyword arguments given that only mean something in the cloud."""
    given = {
        "pool": pool is not None,
        "warm": warm is not None,
        "max_pool_size": max_pool_size is not None,
        "claim_ttl": claim_ttl is not None,
        "claim_spec": claim_spec is not None,
        "api_key": api_key is not None,
        "replicas": replicas != 1,
        "service": service != ENV_SERVICE,
        "region": region != "us-east-1",
        "keep_alive_minutes": keep_alive_minutes is not None,
        "progress": progress is not None,
    }
    return [k for k, v in given.items() if v]


def _requested_placement(kind: Any, runtime: Any) -> str:
    """The explicit ``kind=``/engine ``runtime=`` the caller passed, as text
    (``""`` when neither; ``"auto"`` counts as unset)."""
    parts = []
    if isinstance(kind, str) and kind.strip().lower() not in ("", "auto"):
        parts.append(f"kind={kind.strip().lower()!r}")
    if isinstance(runtime, str) and runtime.strip().lower() not in ("", "auto"):
        parts.append(f"runtime={runtime.strip().lower()!r}")
    return ", ".join(parts)


def _apply_on(
    on: Optional[str],
    local: Optional[bool],
    runtime: Any,
    *,
    cloud: Optional[CloudOptions] = None,
    pool: Any = None,
    cpu: Optional[int] = None,
    memory_mb: Optional[int] = None,
    time_to_start: Optional[float] = None,
) -> "tuple[Optional[str], Optional[bool], Any]":
    """Route ``on=`` to a provider when it names one: a contrib platform
    (``"e2b"``, ``"daytona"``, ``"modal"``; opt-in SDK builds) or your own
    cloud (``"aws"``, ``"gcp"``, ``"modal"``; connect it with ``cua cloud
    connect``). ``"local"``, ``"cloud"``,
    ``"direct:<addr>"`` and ``"relay:<id>"`` fall through to the placement
    model (:mod:`cua_sandbox._placement`). Returns ``(on, local, runtime)``; a
    contrib provider becomes ``(None, True, ContribRuntime(...))``, a local
    sandbox served through the provider's SDK handle."""
    if on is None or not str(on).strip():
        return on, local, runtime
    word = str(on).strip().lower()
    if (
        word in ("local", "cloud", "fleet")
        or word.startswith("direct:")
        or word.startswith("relay:")
    ):
        return on, local, runtime
    from cua_sandbox.runtime.contrib import ContribRuntime, contrib_locations

    if word not in contrib_locations():
        # Not a known provider: let the placement model raise the proper error.
        return on, local, runtime
    if local is not None or runtime is not None or cloud is not None or pool is not None:
        raise InvalidArgument(
            f"on={word!r} picks the provider; drop local=, runtime=, cloud= and pool="
        )
    kwargs: dict[str, Any] = {"ephemeral": True, "cpus": cpu, "memory_mb": memory_mb}
    if time_to_start is not None:
        kwargs["ready_timeout"] = float(time_to_start)
    return None, True, ContribRuntime(word, **kwargs)


def _place_new(
    image: Optional[Image],
    *,
    on: Optional[str],
    local: Optional[bool],
    kind: Optional[str],
    runtime: Any,
    cloud: Optional[CloudOptions],
    cloud_only: list[str],
) -> "tuple[_placement.Placement, Optional[Image]]":
    """Where, what kind and which engine for a new sandbox (see
    :mod:`cua_sandbox._placement`), and the image with the resolved kind."""
    place = _placement.resolve(
        on=on,
        local=local,
        kind=kind,
        runtime=runtime,
        cloud=cloud,
        cloud_only=cloud_only,
        image_kind=image.kind if image is not None else None,
        stacklevel=5,
    )
    if image is not None and place.kind and place.kind != image.kind:
        image = image._with(kind=place.kind)
    return place, image


def _reject_cloud_network(network: Optional[str], local: bool) -> None:
    """``network="none"`` is a local QEMU feature; the cloud refuses it."""
    if network == "none" and not local:
        raise Unsupported(
            "network='none' needs a local QEMU VM (local=True); cloud sandboxes always "
            "have outbound network"
        )


def _sdk_backed(runtime: Any) -> bool:
    """Whether a local adapter starts its sandboxes through the cua SDK (and
    so takes ``kind``/``runtime`` start options)."""
    from cua_sandbox.runtime.native import NativeRuntime
    from cua_sandbox.runtime.qemu import QEMUBaremetalRuntime

    return isinstance(runtime, (NativeRuntime, QEMUBaremetalRuntime))


def _require_network_none_support(runtime: Any) -> None:
    """Local runtimes that can cut guest egress: the SDK's native runtimes
    (the Rust backend refuses containers and Lume itself) and the legacy
    QEMU launchers. Anything else would silently ignore it."""
    from cua_sandbox.runtime.native import NativeRuntime
    from cua_sandbox.runtime.qemu import QEMUBaremetalRuntime, QEMUWSL2Runtime

    if not isinstance(runtime, (NativeRuntime, QEMUBaremetalRuntime, QEMUWSL2Runtime)):
        raise Unsupported(
            f"{type(runtime).__name__} does not support network='none'; only local QEMU "
            "VMs run without outbound network"
        )


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


def _auto_runtime(
    image: Image,
    *,
    cpu: Optional[int] = None,
    memory_mb: Optional[int] = None,
    server_port: Optional[int] = None,
    engine: Optional[str] = None,
) -> "Runtime":
    """Pick a runtime automatically based on image.os_type and image.kind.

    ``cpu`` / ``memory_mb`` size the sandbox; unset keeps each runtime's default.
    ``server_port`` is the image's own server: published, probed for
    readiness and reachable as the ``server`` service. ``engine``
    (``runtime="qemu"``/``"lume"``/``"gvisor"``/``"runc"``) picks the adapter
    for it.
    """
    rt = _pick_runtime(image, cpu=cpu, memory_mb=memory_mb, engine=engine)
    if server_port is not None:
        # NativeRuntime (and the QEMU wrappers that delegate to it) read it.
        rt.server_port = server_port
    return rt


def _pick_runtime(
    image: Image,
    *,
    cpu: Optional[int] = None,
    memory_mb: Optional[int] = None,
    engine: Optional[str] = None,
) -> "Runtime":
    import platform as _plat

    # Docker and Lume name the CPU count ``cpus``; the VM runtimes ``cpu_count``.
    shape = {"cpus": cpu} if cpu is not None else {}
    vm_shape = {"cpu_count": cpu} if cpu is not None else {}
    if memory_mb is not None:
        shape["memory_mb"] = memory_mb
        vm_shape["memory_mb"] = memory_mb

    if image.kind is None:
        raise ValueError(
            "Cannot auto-select runtime: image kind is unresolved. "
            "Either use Image.linux()/windows()/macos() which set kind automatically, "
            "or pass runtime= explicitly for registry images."
        )

    # An engine names its adapter (the SDK checks it against the image).
    if engine == "lume":
        from cua_sandbox.runtime.lume import LumeRuntime

        return LumeRuntime(**shape)
    if engine == "qemu":
        from cua_sandbox.runtime.qemu import QEMURuntime

        return QEMURuntime(mode="bare-metal", **vm_shape)

    if image.kind == "container":
        from cua_sandbox.runtime.docker import DockerRuntime

        return DockerRuntime(ephemeral=True, **shape)

    # kind == "vm"
    if image.os_type == "macos":
        from cua_sandbox.runtime.lume import LumeRuntime

        return LumeRuntime(**shape)

    if image.os_type == "android":
        from cua_sandbox.runtime.android_emulator import AndroidEmulatorRuntime

        return AndroidEmulatorRuntime(**vm_shape)

    if image.os_type == "windows" and _plat.system() == "Windows":
        from cua_sandbox.runtime.hyperv import _has_hyperv

        if _has_hyperv():
            from cua_sandbox.runtime.hyperv import HyperVRuntime

            return HyperVRuntime(**vm_shape)

    # If image has a disk path (from_file), use bare-metal QEMU
    if image._disk_path:
        from cua_sandbox.runtime.qemu import QEMURuntime

        return QEMURuntime(mode="bare-metal", **vm_shape)

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
        return QEMURuntime(mode="bare-metal", **vm_shape)

    if image.os_type == "windows":
        # Windows bare-metal QEMU works on any host with qemu-system-x86_64
        try:
            from cua_sandbox.runtime.docker import _has_docker

            if not _has_docker():
                return QEMURuntime(mode="bare-metal", **vm_shape)
        except Exception:
            pass

    return QEMURuntime(mode="docker", **vm_shape)


# Coarse telemetry labels for built-in runtime classes. Anything else (user
# subclasses, contributed runtimes) is reported as "other" so a user-chosen
# class name never leaves the machine.
_TELEMETRY_RUNTIME_LABELS = {
    "DockerRuntime": "docker",
    "LumeRuntime": "lume",
    "NativeQEMURuntime": "qemu",
    "QEMUDockerRuntime": "qemu-docker",
    "QEMUBaremetalRuntime": "qemu-baremetal",
    "QEMUWSL2Runtime": "qemu-wsl2",
    "HyperVRuntime": "hyperv",
    "TartRuntime": "tart",
    "AndroidEmulatorRuntime": "android-emulator",
    "NativeRuntime": "native",
    "ContribRuntime": "contrib",
}
_TELEMETRY_OS_TYPES = {"linux", "macos", "windows", "android"}
_TELEMETRY_IMAGE_KINDS = {"container", "vm"}


def _telemetry_runtime_label(runtime: Any) -> str:
    cls = type(runtime)
    label = _TELEMETRY_RUNTIME_LABELS.get(cls.__name__)
    if label and (cls.__module__ or "").startswith("cua_sandbox."):
        return label
    return "other"


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
    # Never send the sandbox name: it is user-chosen.
    props: dict = {
        "local": local,
        "ephemeral": ephemeral,
        "duration_seconds": round(time.monotonic() - t_start, 3),
    }
    if image is not None:
        os_type = getattr(image, "os_type", None)
        kind = getattr(image, "kind", None)
        props["os_type"] = os_type if os_type in _TELEMETRY_OS_TYPES else "other"
        if kind is not None:
            props["image_kind"] = kind if kind in _TELEMETRY_IMAGE_KINDS else "other"
    if sb._runtime is not None:
        props["runtime_type"] = _telemetry_runtime_label(sb._runtime)
    record_event("sandbox_create", props)


class Sandbox:
    """A sandboxed computer environment.

    Provides programmatic control of a VM or container through a unified
    interface: ``.mouse``, ``.keyboard``, ``.screen``, ``.clipboard``,
    ``.shell``, ``.window``, and ``.terminal``.

    Sandboxes are always isolated — they never control the host machine
    directly. To control the local machine, use cua-driver.

    There are three ways to obtain a Sandbox:

    1. **Persistent** — provision and keep alive after the script exits::

           sb = await Sandbox.create(Image.linux())
           await sb.shell.run("whoami")
           await sb.disconnect()

    2. **Connect** — attach to an already-running sandbox by name::

           sb = await Sandbox.connect("my-sandbox")
           await sb.screenshot()
           await sb.disconnect()

    3. **Ephemeral** — auto-destroyed when the ``async with`` block exits::

           async with Sandbox.ephemeral(Image.linux()) as sb:
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
        """The sandbox name (`None` until a cloud sandbox is bound)."""
        self._runtime = _runtime
        self._runtime_info = _runtime_info
        self._ephemeral = _ephemeral
        self._has_snapshots = False
        self._claim_handle: Any = None
        self._claim_released = False
        # Image info the Python side resolved (runtimes with no SDK handle).
        self._image_info_fallback: Optional[ImageInfo] = None
        # Set for a machine reached by URL (``direct:<host:port>``).
        self._direct_url: Optional[str] = None
        self.telemetry_enabled = _telemetry_enabled
        """Whether this sandbox records anonymous usage events."""
        self.screen = Screen(transport)
        """Screenshots and screen size (cua-spacesd)."""
        self.mouse = Mouse(transport)
        """Pointer input in screen pixels (cua-spacesd)."""
        self.keyboard = Keyboard(transport)
        """Typing and key presses (cua-spacesd)."""
        self.clipboard = Clipboard(transport)
        """Clipboard text (cua-spacesd)."""
        self.shell = Shell(transport)
        """Run shell commands (cua-spacesd)."""
        self.files = Files(transport)
        """Read, write and transfer files (cua-spacesd)."""
        self.window = Window(transport)
        """The active window (cua-spacesd)."""
        self.terminal = Terminal(transport)
        """PTY terminal sessions (cua-spacesd)."""
        self.mobile = Mobile(transport)
        """Android touch and hardware keys (Android sandboxes)."""
        self.tunnel = Tunnel(transport)
        """Forward sandbox ports to loopback ports on this machine."""
        self.services = Services(transport)
        """Older named-service helpers; prefer `service(name)`."""
        self.driver = Driver(transport)
        """Typed Cua Driver access (`cua-sandbox[driver]`)."""
        _os = _runtime_info.environment if _runtime_info and _runtime_info.environment else "linux"
        self.apps = Apps(transport, os_type=_os)
        """Install and launch catalog applications (cua-spacesd)."""

    async def _connect(self) -> None:
        await self._transport.connect()
        # Update name from transport (e.g. CloudTransport resolves name after creating a VM)
        if self.name is None and isinstance(self._transport, (CloudTransport, FleetCloudTransport)):
            self.name = self._transport.name

    async def disconnect(self) -> None:
        """Drop the transport connection. The sandbox keeps running.

        A claim on a managed Fleet pool stops being renewed and lives until
        its shutdown time (``claim_ttl``, or later if ``keep_alive`` was
        called).
        """
        detach = getattr(self._claim_handle, "detach", None)
        if callable(detach):
            detach()
        await self.driver.close()
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
    def image_info(self) -> Optional[ImageInfo]:
        """The image this sandbox runs, as resolved and pinned at create time
        (digest, variant, arch), or ``None`` when unknown: direct (URL)
        connections, images not resolved from a registry. A claim on a named
        Fleet pool reports its template's image (``pinned_ref``/``digest``
        empty when the registry could not be read).

        The SDK handle's ``image_info()`` is the source of truth; runtimes
        without one report what was resolved at create or claim time. Never
        makes a network call and never raises.
        """
        try:
            for handle in self._native_candidates():
                getter = getattr(handle, "image_info", None)
                if callable(getter):
                    return ImageInfo._from_native(getter())
            return getattr(self, "_image_info_fallback", None)
        except Exception:  # noqa: BLE001 - informational; never fail the caller
            return None

    def _native_candidates(self) -> list:
        """SDK sandbox handles this sandbox holds, without any I/O."""
        info = getattr(self, "_runtime_info", None)
        claim = getattr(self, "_claim_handle", None)
        transport = getattr(self, "_transport", None)
        found = [
            getattr(info, "native", None) if info is not None else None,
            getattr(claim, "_native", None) if claim is not None else None,
            getattr(transport, "_native_fleet_sandbox", None),
            getattr(transport, "_native_sandbox", None),
        ]
        return [h for h in found if h is not None]

    @property
    def claim_name(self) -> str | None:
        """Fleet claim name, distinct from the bound sandbox name."""
        return self._claim_handle.name if self._claim_handle is not None else None

    @property
    def pool_name(self) -> str | None:
        """Fleet pool that owns this claim (advanced; see
        ``(await sb.info()).provider_details``)."""
        return self._claim_handle.pool_name if self._claim_handle is not None else None

    @property
    def location(self) -> str:
        """Where it runs: ``local``, ``cloud`` or ``direct``."""
        if self._direct_url is not None:
            return "direct"
        if self._claim_handle is not None or isinstance(
            self._transport, (CloudTransport, FleetCloudTransport)
        ):
            return "cloud"
        return "local"

    @property
    def id(self) -> str | None:
        """The qualified ref, the same kind of value local and in the cloud:
        ``local:<name>``, ``cloud:<name>`` or ``direct:<host:port>``. It is
        what ``Sandbox.connect(id)`` and ``Sandbox.delete(id)`` take, and
        what listings print."""
        if self._direct_url is not None:
            try:
                return _refs.parse(self._direct_url).id
            except Exception:  # noqa: BLE001 - an unparsable URL keeps its name
                return self.name
        name = self.claim_name or self.name
        return _refs.qualified(self.location, name) if name else None

    async def _native_handle(self) -> Any:
        """The ``cua.Sandbox`` handle (services, forwards, public URLs)."""
        handle = getattr(self._transport, "native_handle", None)
        if callable(handle):
            return await handle()
        native = getattr(self._runtime_info, "native", None) if self._runtime_info else None
        if native is not None:
            return native
        raise NotImplementedError(
            f"{type(self._transport).__name__} sandboxes have no named services"
        )

    async def overlay(
        self, overlay: "dict[str, Any] | list", timeout: Optional[float] = None
    ) -> list:
        """Injects binaries into this sandbox (see ``create(overlay=...)``):
        ``{"cua-driver": "./target/release/cua-driver"}``. Returns one
        ``cua.OverlayResult`` per binary (``name``, ``target``, ``sha256``,
        ``previous_sha256``, ``restarted``); pass ``NAME=sha256:<hex>`` to
        ``cua doctor --expect``. ``timeout`` (seconds) bounds a cua-spacesd
        restart."""
        handle = await self._native_handle()
        ms = None if timeout is None else int(timeout * 1000)
        return list(await handle.overlay(_overlay_specs(overlay), ms))

    async def info(self) -> SandboxInfo:
        """Portable info: ``status`` (provisioning, starting, ready, stopped),
        ``location`` (local, cloud or direct), ``services`` (name -> guest
        port), ``expires_at`` (cloud) and ``provider_details``."""
        handle = await self._native_handle()
        record = await handle.refresh()
        expires = (
            datetime.fromtimestamp(record.expires_at_unix, timezone.utc).isoformat()
            if record.expires_at_unix
            else None
        )
        phase = _PHASE_WORDS.get(getattr(record.phase, "name", str(record.phase)), "starting")
        return SandboxInfo(
            name=self.name or record.name,
            status=phase,
            source="cloud" if record.location == "cloud" else record.runtime_type,
            os_type=getattr(self._runtime_info, "environment", None),
            id=record.id or self.id,
            location=record.location,
            services={k: v for k, v in dict(record.services).items() if v or k != "env"},
            expires_at=expires,
            provider_details=dict(record.provider_details),
            kind=getattr(record, "kind", None) or None,
            runtime=getattr(record, "runtime", None) or None,
        )

    def service(self, name: str) -> "ServiceHandle":
        """A named service (``services={"mcp": 8765}`` at create):
        ``request``, ``url`` and ``public_url``, the same local and in the
        cloud."""
        from cua_sandbox.interfaces.services import ServiceHandle

        return ServiceHandle(self, name)

    async def public_url(
        self, service: str, *, ttl: float = 3600, label: Optional[str] = None
    ) -> PublicUrl:
        """A shareable URL for ``service`` that stops working after ``ttl``
        seconds (60 s to 24 h). Cloud: a signed service URL. Local: a
        loopback URL with its own token, served by the cua daemon (started
        if needed)."""
        from cua_sandbox.interfaces.services import public_url_from_native

        handle = await self._native_handle()
        return public_url_from_native(await handle.public_url(service, int(ttl), label))

    async def revoke_public_url(self, url: "PublicUrl | str") -> None:
        """Revokes a URL from :meth:`public_url` (or its id)."""
        handle = await self._native_handle()
        await handle.revoke_public_url(url if isinstance(url, str) else url.id)

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
        await self.driver.close()
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
            record_event("sandbox_destroy", {"ephemeral": self._ephemeral})
        if self._claim_handle is not None and not self._claim_released:
            # A Fleet claim (managed or explicit pool): release it; managed
            # pools themselves are reused and reclaimed by idle GC.
            try:
                await self.close()
            except Exception:
                logger.warning("Failed to release Fleet claim for sandbox %r", self.name)
            return
        # Run each cleanup step independently so a failure in one
        # (e.g. disconnect timeout) doesn't prevent the VM from being deleted.
        try:
            await self.disconnect()
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

        With cua-spacesd this is a link to its HTML5 viewer, carrying a
        scoped viewer ticket that expires. Images without cua-spacesd return
        the page of a declared web display service, or a VNC address.

        Args:
            share: If True, return a link meant for sharing (an expiring
                   public link for a legacy web display service; viewer links
                   always expire). If False, return a direct connection URL.
        """
        return await self._transport.get_display_url(share=share)

    async def viewer_url(self) -> str:
        """A browser link to this sandbox's desktop in the cua-spacesd HTML5
        viewer (video, audio, input, clipboard, files). The link carries a
        scoped viewer ticket that expires after an hour. Same as
        ``get_display_url()`` for sandboxes with cua-spacesd.
        """
        return await self._transport.get_display_url(share=False)

    async def get_dimensions(self) -> tuple[int, int]:
        return await self.screen.size()

    def mcp(self, service: str, *, path: str = "/mcp") -> Any:
        """An official MCP SDK client for the MCP server behind ``service``.

        An async context manager (``pip install "cua-sandbox[mcp]"``)::

            async with sb.mcp("mcp") as client:
                tools = await client.list_tools()
                result = await client.call_tool("add", {"a": 2, "b": 3})

        Requests go through the sandbox's service route (loopback locally, the
        Fleet gateway in the cloud), streamed both ways, so every protocol
        feature and content block works unchanged. No cua-spacesd needed.
        """
        return _mcp.open_mcp(lambda: self._transport.native_service(service), path)

    async def mcp_config(self, service: str, *, path: str = "/mcp") -> dict:
        """``{"url": ..., "headers": {...}}`` of ``service``'s MCP endpoint, for
        any MCP client (Claude Code, Cursor, other SDKs). The headers can carry
        a short-lived Fleet bearer: fetch a fresh config per connection."""
        return await _mcp.mcp_config(lambda: self._transport.native_service(service), path)

    async def spacesd(self) -> Any:
        """The raw ``cua.SpacesdClient`` for this sandbox's cua-spacesd.

        The typed escape hatch to every ``cua.env.v1`` RPC (``call_json``),
        process streaming, chunked transfers and media sessions. Raises
        :class:`~cua_sandbox._sdk.SpacesdNotAvailable` when the sandbox has no
        spacesd (for example a plain VNC-only image).
        """
        if isinstance(self._transport, EnvTransport):
            return await self._transport.spacesd()
        raise SpacesdNotAvailable(
            f"{type(self._transport).__name__} sandboxes are driven without cua-spacesd"
        )

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
        service: str = ENV_SERVICE,
        claim_spec: Any = None,
        keep_alive_minutes: float | None = None,
        api_key: Optional[str] = None,
        on: Optional[str] = None,
        local: Optional[bool] = None,
        kind: Optional[str] = None,
        runtime: "Runtime | str | None" = None,
        cpu: Optional[int] = None,
        memory_mb: Optional[int] = None,
        disk_gb: Optional[int] = None,
        region: str = "us-east-1",
        time_to_start: Optional[float] = None,
        request_timeout: Optional[float] = None,
        server_port: Optional[int] = None,
        telemetry_enabled: bool = True,
        warm: Optional[bool] = None,
        max_pool_size: Optional[int] = None,
        claim_ttl: "float | timedelta | None" = None,
        progress: Optional[Callable[[Any], Any]] = None,
        command: Optional[list[str]] = None,
        sidecars: Optional[list] = None,
        env: Optional[dict[str, str]] = None,
        services: Optional[dict[str, int]] = None,
        wait_for: WaitFor = None,
        cloud: Optional[CloudOptions] = None,
        memory: "str | int | None" = None,
        network: Optional[str] = None,
        overlay: "dict[str, Any] | list | None" = None,
    ) -> "Sandbox":
        """Provision or claim a persistent sandbox and return it connected.

        Where, what kind and which engine are three separate choices:

        * ``on``: ``"local"`` or ``"cloud"``. ``local=True``/``False`` is the
          same switch; ``on`` and a contradicting ``local`` is an
          :class:`InvalidArgument`, and so is ``local=True`` with ``cloud=``.
          Unset, ``cloud=`` implies the cloud; a cloud-only argument (a
          Fleet ``pool``, ``warm``, ``max_pool_size``, ``claim_ttl``,
          ``claim_spec``, ``api_key``, ...) still implies it with a
          ``DeprecationWarning``; otherwise the user default applies
          (``CUA_DEFAULT_ON``, else ``cua config set default.on cloud`` in
          ``$CUA_HOME/config.toml``, else local, with a one-time notice that
          ``CUA_QUIET_DEFAULT=1`` hides).
        * ``kind``: ``"auto"``, ``"container"`` or ``"vm"``. Unset: the
          image's ``kind`` when it has one, else ``default.kind``, else auto
          (macOS and Windows images are VMs, a container rootfs is a
          container, a disk-only image is a VM).
        * ``runtime``: the engine, ``"auto"`` or one the location offers for
          the kind: locally ``"gvisor"``/``"runc"`` (containers) and
          ``"qemu"``/``"lume"`` (VMs); in the cloud ``"gvisor"`` and
          ``"kubevirt"``. Unset: ``default.runtime``, else auto. A local
          sandbox with ``sidecars`` needs ``"runc"`` where gVisor would run
          (separate gVisor containers cannot share a network namespace).
          A :class:`Runtime` object (``DockerRuntime()``, ``TartRuntime()``,
          ...) still picks a local adapter, as before.

        A combination that does not exist (``kind="container",
        runtime="qemu"``, or ``runtime="kubevirt"`` locally) raises
        :class:`InvalidPlacement`, listing the valid values.

        The same options mean the same thing local and in the cloud:

        * ``command``: argv replacing the image's entrypoint.
        * ``env``: environment variables.
        * ``services``: named guest ports, ``{"mcp": 8765}``; reach them with
          ``sb.service("mcp")``, ``sb.public_url("mcp")`` or
          ``sb.tunnel.forward(8765)``.
        * ``wait_for``: ``tcp("mcp")`` / ``http("mcp", "/health")`` (or a
          list); readiness otherwise is "the sandbox is running".
        * ``cloud``: :class:`CloudOptions` (warm capacity, limits, TTL, a
          dedicated pool). The flat ``pool``, ``warm``, ``max_pool_size`` and
          ``claim_ttl`` keywords are deprecated aliases.
        * ``memory``: ``"4GB"`` / ``"512MB"`` (or ``memory_mb``).
        * ``network``: ``"default"`` (outbound network, like a Docker
          container) or ``"none"`` (no egress; the SDK still reaches the
          guest's published ports). ``"none"`` needs a local QEMU VM;
          containers, Lume and cloud sandboxes raise :class:`Unsupported`.

        * ``overlay``: binaries to inject once it is up, so tests run the
          build under test and not the copy the image bundles:
          ``{"cua-driver": "./target/release/cua-driver"}`` (``cua-driver`` and
          ``cua-spacesd`` resolve their guest path; other names take
          ``(path, "/guest/path")``). Each replaces the guest file atomically,
          is recorded with its sha256 (``cua doctor --expect``) and what runs
          it is restarted. If it fails, the sandbox is deleted and the error
          raised. See :meth:`overlay`.

        Supplying ``pool`` claims from an existing Fleet pool without changing
        its configuration.

        A Fleet registry image without ``pool`` is claimed from this
        account's managed pool for that image spec (``cua-auto-*``), created
        on first use and reused afterwards. The first start of an image can
        take a few minutes; later starts reuse its capacity. Fleet-only
        options:

        * ``warm``: seed one ready replica when the pool is first created.
        * ``max_pool_size``: autoscaling ceiling (default 10).
        * ``claim_ttl``: seconds (or a timedelta) the claim outlives this
          process; it is renewed while the process holds it (default 15 min).
          Use ``keep_alive(minutes=...)`` to keep a sandbox longer.
        * ``progress``: callback receiving ``AcquireProgress`` events.

        Without ``on``/``local``, passing one of them keeps the sandbox in
        the cloud (deprecated: pass ``local=False``); with ``local=True`` they
        are ignored. An existing ``pool`` keeps its own kind and runtime
        (``Pool.apply(..., runtime=)``).

        ``server_port`` names a port your image serves itself: it is exposed
        (Fleet: as the ``server`` service) and becomes the readiness probe.
        Leave it unset for the default, daemon-agnostic readiness (the
        provider reports the sandbox running; cua-spacesd on 3211 is used
        by the interfaces when the image has it).
        """
        from cua_sandbox.pool import Pool

        validate_server_port(server_port)
        on, local, runtime = _apply_on(
            on,
            local,
            runtime,
            cloud=cloud,
            pool=pool,
            cpu=cpu,
            memory_mb=memory_mb,
            time_to_start=time_to_start,
        )
        asked = _requested_placement(kind, runtime)
        place, image = _place_new(
            image,
            on=on,
            local=local,
            kind=kind,
            runtime=runtime,
            cloud=cloud,
            cloud_only=_cloud_only_args(
                pool=pool,
                warm=warm,
                max_pool_size=max_pool_size,
                claim_ttl=claim_ttl,
                claim_spec=claim_spec,
                api_key=api_key,
                replicas=replicas,
                service=service,
                region=region,
                keep_alive_minutes=keep_alive_minutes,
                progress=progress,
            ),
        )
        local = place.local
        # The engine (a string) and a legacy local Runtime object.
        engine, runtime = place.runtime, place.legacy_runtime
        network = check_network(network)
        _reject_cloud_network(network, local)
        pool, warm, max_pool_size, claim_ttl = _merge_cloud(
            cloud,
            pool=pool,
            warm=warm,
            max_pool_size=max_pool_size,
            claim_ttl=claim_ttl,
        )
        if memory is not None:
            if memory_mb is not None:
                raise ValueError("pass memory= or memory_mb=, not both")
            memory_mb = parse_memory(memory)
        wait = probes(wait_for)
        services = check_services(services, wait)

        apply_template = bool(cloud is not None and cloud.apply)
        if apply_template and pool is None:
            raise ValueError("CloudOptions(apply=True) updates a named pool; pass pool= too")
        if pool is not None:
            if wait:
                raise ValueError(
                    "wait_for belongs to the image's template; an existing pool keeps its own "
                    "readiness (omit the pool to get a sandbox with it)"
                )
            if replicas != 1 or disk_gb is not None:
                raise ValueError("configuration cannot be supplied for an existing pool")
            pool_name = pool if isinstance(pool, str) else pool.name
            # The given sandbox fields must be the pool's template (never
            # silently ignored), or apply=True updates it.
            requested = SandboxSpec(
                image=image,
                command=command,
                env=dict(env or {}),
                services=dict(services or {}),
                cpu=cpu,
                memory_mb=memory_mb,
                sidecars=list(sidecars or ()),
            )
            if image is not None or command or env or services or sidecars or cpu or memory_mb:
                if apply_template:
                    if pool_name.startswith("cua-auto-"):
                        raise ValueError(
                            f"{pool_name} is a managed pool (its template is its key); "
                            "apply=True updates only pools you own (omit the pool to get a "
                            "sandbox with these fields)"
                        )
                    await Pool.apply_template(pool_name, requested)
                else:
                    await Pool.check(pool_name, requested)
            _reject_managed_options_for_pool(warm, max_pool_size, claim_ttl)
            if asked:
                raise ValueError(
                    f"an existing pool already has its kind and runtime ({asked}); pass "
                    "runtime= to Pool.apply(image, name=..., runtime=...) instead"
                )
            if (
                local
                or runtime is not None
                or api_key is not None
                or region != "us-east-1"
                or request_timeout is not None
                or server_port is not None
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
            return await _with_overlays(sandbox, overlay)

        from cua_sandbox.image import cloud_registry_image

        fleet_image = (
            image is not None
            and cloud_registry_image(image) is not None
            and cls._uses_fleet(api_key)
            and not local
            and runtime is None
        )
        if fleet_image:
            assert image is not None
            _check_managed_options(
                replicas=replicas,
                claim_spec=claim_spec,
                disk_gb=disk_gb,
                region=region,
                request_timeout=request_timeout,
            )
            sandbox = await _acquire_managed(
                image,
                name=name,
                service=service,
                cpu=cpu,
                memory_mb=memory_mb,
                server_port=server_port,
                time_to_start=time_to_start,
                warm=warm,
                max_pool_size=max_pool_size,
                claim_ttl=claim_ttl,
                progress=progress,
                telemetry_enabled=telemetry_enabled,
                fleet_runtime=engine,
                command=command,
                sidecars=sidecars,
                env=env,
                services=services,
                wait_for=wait,
                hint=place.cloud_default_hint(),
            )
            await _keep_alive_or_close(sandbox, keep_alive_minutes)
            await _save_fleet_claim_or_close(
                sandbox,
                sandbox.claim_name,
                sandbox.pool_name,
                **sandbox._claim_handle.state_fields(),
            )
            return await _with_overlays(sandbox, overlay)

        if image is None:
            raise ValueError("image is required when pool is omitted")
        if not local and engine is not None and "runtime" in asked:
            raise ValueError(
                f"runtime={engine!r} in the cloud applies to registry images only "
                "(Image.from_registry, Image.linux(), Image.windows(), Image.macos())"
            )
        if replicas != 1 or service != ENV_SERVICE or claim_spec is not None:
            raise NotImplementedError("claim options are only supported by Fleet")
        created = await cls._create(
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
            command=command,
            sidecars=sidecars,
            engine=engine,
            network=network,
            env=env,
            services=services,
            wait_for=wait,
            hint=place.cloud_default_hint(),
        )
        return await _with_overlays(created, overlay)

    @classmethod
    def connect(
        cls,
        name: Optional[str] = None,
        *,
        url: Optional[str] = None,
        token: Optional[str] = None,
        api_key: Optional[str] = None,
        local: Optional[bool] = None,
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
            name: A sandbox ref (``local:<name>``, ``cloud:<name>``,
                ``direct:<host:port>``, a legacy id) or a name. A bare name
                must be unique across locations, else
                :class:`AmbiguousSandbox` lists the refs it matches.
            local: ``None`` (default): search every location. ``True``/
                ``False`` narrow a bare name to local or cloud.
            url: A cua-spacesd URL to connect to directly
                (``http://host:3211``, a relay URL or a Fleet service URL).
            token: The spacesd token for ``url``.
            api_key: Legacy CUA API key (API-key cloud VMs were removed; see
                :mod:`cua_sandbox.transport.cloud`).
            ws_url: Removed (the computer-server WebSocket protocol is gone).
            http_url: Base URL of a cua-spacesd (same as ``url``).
            container_name: Unused; kept for compatibility.
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

        if name is None and url is None and http_url is None:
            raise ValueError("Sandbox.connect needs a sandbox name or url=")

        async def _factory() -> "Sandbox":
            # A ref decides the location; a bare name is the local sandbox
            # when one exists (and no cloud sandbox has the name too, else
            # AmbiguousSandbox), else the cloud.
            target, is_local, direct = name, bool(local), None
            if name and not (url or http_url):
                target, is_local, direct = await _refs.resolve(name, local)
            return await cls._create(
                name=target if direct is None else None,
                url=url or direct,
                token=token,
                ephemeral=False,
                local=bool(is_local),
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
        service: str = ENV_SERVICE,
        claim_spec: Any = None,
        keep_alive_minutes: float | None = None,
        keep_pool: bool = False,
        api_key: Optional[str] = None,
        on: Optional[str] = None,
        local: Optional[bool] = None,
        kind: Optional[str] = None,
        runtime: "Runtime | str | None" = None,
        cpu: Optional[int] = None,
        memory_mb: Optional[int] = None,
        disk_gb: Optional[int] = None,
        region: str = "us-east-1",
        time_to_start: Optional[float] = None,
        request_timeout: Optional[float] = None,
        server_port: Optional[int] = None,
        telemetry_enabled: bool = True,
        warm: Optional[bool] = None,
        max_pool_size: Optional[int] = None,
        claim_ttl: "float | timedelta | None" = None,
        progress: Optional[Callable[[Any], Any]] = None,
        command: Optional[list[str]] = None,
        sidecars: Optional[list] = None,
        env: Optional[dict[str, str]] = None,
        services: Optional[dict[str, int]] = None,
        wait_for: WaitFor = None,
        cloud: Optional[CloudOptions] = None,
        memory: "str | int | None" = None,
        network: Optional[str] = None,
    ) -> AsyncIterator["Sandbox"]:
        """A sandbox that is released when the ``async with`` block exits.

        Takes the same ``on``/``local``, ``kind``, ``runtime``, ``command``,
        ``env``, ``services``, ``wait_for``, ``cloud``, ``memory`` and
        ``network`` options as :meth:`create`.

        A Fleet registry image without ``pool`` is claimed from this
        account's reusable managed pool (see :meth:`create` for ``warm``,
        ``max_pool_size``, ``claim_ttl`` and ``progress``). Exiting releases
        the claim; the pool stays for the next run and idle GC deletes it
        once unused. If the process dies, the claim expires after
        ``claim_ttl``. ``keep_pool`` is deprecated and has no effect.
        """
        from cua_sandbox.image import cloud_registry_image

        on, local, runtime = _apply_on(
            on,
            local,
            runtime,
            cloud=cloud,
            pool=pool,
            cpu=cpu,
            memory_mb=memory_mb,
            time_to_start=time_to_start,
        )
        asked = _requested_placement(kind, runtime)
        place, image = _place_new(
            image,
            on=on,
            local=local,
            kind=kind,
            runtime=runtime,
            cloud=cloud,
            cloud_only=_cloud_only_args(
                pool=pool,
                warm=warm,
                max_pool_size=max_pool_size,
                claim_ttl=claim_ttl,
                claim_spec=claim_spec,
                api_key=api_key,
                replicas=replicas,
                service=service,
                region=region,
                keep_alive_minutes=keep_alive_minutes,
                progress=progress,
            ),
        )
        local = place.local
        # The engine (a string) and a legacy local Runtime object.
        engine, runtime = place.runtime, place.legacy_runtime
        network = check_network(network)
        _reject_cloud_network(network, local)
        pool, warm, max_pool_size, claim_ttl = _merge_cloud(
            cloud,
            pool=pool,
            warm=warm,
            max_pool_size=max_pool_size,
            claim_ttl=claim_ttl,
        )
        if memory is not None:
            if memory_mb is not None:
                raise ValueError("pass memory= or memory_mb=, not both")
            memory_mb = parse_memory(memory)
        wait = probes(wait_for)
        services = check_services(services, wait)
        custom = bool(command) or bool(env) or bool(services) or bool(wait) or bool(sidecars)

        fleet_image = (
            image is not None
            and cloud_registry_image(image) is not None
            and cls._uses_fleet(api_key)
            and not local
            and runtime is None
        )
        if keep_pool:
            warnings.warn(
                "keep_pool is deprecated and has no effect: managed Fleet pools are "
                "reused automatically and deleted by idle GC once unused",
                DeprecationWarning,
                stacklevel=3,
            )
        if asked and pool is not None:
            raise ValueError(
                f"an existing pool already has its kind and runtime ({asked}); pass "
                "runtime= to Pool.apply(image, name=..., runtime=...) instead"
            )
        if not local and "runtime" in asked and not fleet_image:
            raise ValueError(
                f"runtime={engine!r} in the cloud applies to registry images only "
                "(Image.from_registry, Image.linux(), Image.windows(), Image.macos())"
            )

        if pool is not None:
            if custom:
                raise ValueError(
                    "command, env, services, sidecars and wait_for belong to the image's "
                    "template; an existing pool keeps its own (omit the pool to get a sandbox "
                    "with them)"
                )
            _reject_managed_options_for_pool(warm, max_pool_size, claim_ttl)
            sandbox = await cls.create(
                image,
                cloud=CloudOptions(pool=pool),
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
            assert image is not None
            _check_managed_options(
                replicas=replicas,
                claim_spec=claim_spec,
                disk_gb=disk_gb,
                region=region,
                request_timeout=request_timeout,
            )
            sandbox = await _acquire_managed(
                image,
                name=name,
                service=service,
                cpu=cpu,
                memory_mb=memory_mb,
                server_port=server_port,
                time_to_start=time_to_start,
                warm=warm,
                max_pool_size=max_pool_size,
                claim_ttl=claim_ttl,
                progress=progress,
                telemetry_enabled=telemetry_enabled,
                ephemeral=True,
                fleet_runtime=engine,
                command=command,
                sidecars=sidecars,
                env=env,
                services=services,
                wait_for=wait,
                hint=place.cloud_default_hint(),
            )
            await _keep_alive_or_close(sandbox, keep_alive_minutes)
            try:
                yield sandbox
            except BaseException:
                await _cleanup_ephemeral_fleet(sandbox, None, suppress_errors=True)
                raise
            else:
                await _cleanup_ephemeral_fleet(sandbox, None, suppress_errors=False)
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
            command=command,
            sidecars=sidecars,
            engine=engine,
            network=network,
            env=env,
            services=services,
            wait_for=wait,
            hint=place.cloud_default_hint(),
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
        location: Optional[str] = None,
        local: Optional[bool] = None,
        all: Optional[bool] = None,  # noqa: A002 - deprecated
        api_key: Optional[str] = None,
    ) -> "list[SandboxInfo]":
        """List sandboxes, each with its ``location``, ``kind`` and
        ``runtime``: local and cloud ones by default.

        Args:
            location: ``None`` (default): local and cloud. ``"local"``: only
                the local ones (containers, QEMU, Lume, Android).
                ``"cloud"``: only the cloud ones.
            local: The same switch as a bool (``True`` is ``"local"``,
                ``False`` is ``"cloud"``).
            all: Deprecated; everything is the default.
            api_key: Legacy CUA API key for api.cua.ai VMs.

        The cloud part never fails the default listing: without cloud
        credentials it is left out silently, and when the cloud fails or does
        not answer within 5 s the local rows come back with a logged warning
        ("cloud sandboxes not listed: ...").
        """
        if all is not None:
            warnings.warn(
                "Sandbox.list(all=...) is deprecated: local and cloud is the default",
                DeprecationWarning,
                stacklevel=2,
            )
            if all and (local is not None or location is not None):
                raise InvalidArgument("pass all=True or local=/location=, not both")
        if location is not None:
            where = str(location).strip().lower()
            if where not in _placement.LOCATIONS:
                raise InvalidArgument(
                    f"location={location!r}: cua-sandbox lists 'local' and 'cloud' sandboxes"
                )
            if local is not None and (where == "local") != bool(local):
                raise InvalidArgument(
                    f"location={location!r} and local={local!r} contradict each other"
                )
            local = where == "local"
        if local is True:
            return await cls._list_local()
        if local is False:
            return await cls._list_cloud(api_key=api_key)
        rows = await cls._list_local()
        from cua_sandbox._config import (
            get_api_key,
            has_fleet_auth,
            may_have_fleet_session,
        )

        if not (api_key or has_fleet_auth() or get_api_key() or may_have_fleet_session()):
            # No cloud credentials that can be read without the OS keychain:
            # nothing to list in the cloud (``local=False`` also uses a
            # keychain session).
            return rows
        try:
            cloud = await asyncio.wait_for(
                cls._list_cloud(api_key=api_key), timeout=_LIST_CLOUD_TIMEOUT
            )
        except asyncio.TimeoutError:
            logger.warning(
                "cloud sandboxes not listed: the cloud did not answer within %ss",
                int(_LIST_CLOUD_TIMEOUT),
            )
            cloud = []
        except Exception as error:  # noqa: BLE001 - the cloud never fails the listing
            if "credentials missing" not in str(error).lower():
                logger.warning("cloud sandboxes not listed: %s", error)
            cloud = []
        # One row per ref: a local and a cloud sandbox may share a name.
        ids = {r.id for r in rows}
        return rows + [r for r in cloud if r.id not in ids]

    @classmethod
    async def _list_local(cls) -> "list[SandboxInfo]":
        import asyncio

        from cua_sandbox.runtime.android_emulator import AndroidEmulatorRuntime

        async def _list_sdk():
            # Every sandbox the SDK knows: state files (including those older
            # cua-sandbox releases wrote) plus instances its backends manage.
            try:
                from cua_sandbox._sdk import local_runtime

                return list(await local_runtime().sandboxes().list("local"))
            except Exception as error:  # noqa: BLE001 - listing is best effort
                logger.debug("SDK local listing failed: %s", error)
                return []

        async def _list_android():
            try:
                return await AndroidEmulatorRuntime().list()
            except Exception:
                return []

        sdk_records, android_vms = await asyncio.gather(_list_sdk(), _list_android())

        from cua_sandbox import sandbox_state
        from cua_sandbox.runtime.native import _status_word

        results: list[SandboxInfo] = []
        seen: set[str] = set()
        for record in sdk_records:
            state = sandbox_state.load(record.name) or {}
            host = state.get("host")
            api_port = state.get("api_port")
            results.append(
                SandboxInfo(
                    name=record.name,
                    status=(
                        _status_word(record.status)
                        if record.status_detail is None
                        else state.get("status", record.status_detail)
                    ),
                    source=record.runtime_type,
                    os_type=state.get("os_type"),
                    host=host,
                    api_url=f"http://{host}:{api_port}" if host and api_port else None,
                    created_at=state.get("created_at"),
                    id=record.id or _refs.qualified("local", record.name),
                    location="local",
                    services=dict(state.get("services") or {}),
                    provider_details={"backend": record.runtime_type},
                    kind=getattr(record, "kind", None) or None,
                    runtime=getattr(record, "runtime", None) or None,
                )
            )
            seen.add(record.name)
        for vm in android_vms:
            if vm["name"] in seen:
                continue
            results.append(
                SandboxInfo(
                    location="local",
                    id=_refs.qualified("local", vm["name"]),
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
        """Fleet for every call without an explicit (legacy) API key: with
        client credentials or a token, else the ``cua auth login`` session;
        with neither, the Fleet path raises "Fleet credentials missing"."""
        if api_key is not None:
            return False
        if has_fleet_access():
            return True
        from cua_sandbox._config import get_api_key

        # A legacy api.cua.ai key only lists/deletes old VMs; without one
        # (and without Fleet credentials) Fleet explains what to configure.
        return get_api_key() is None

    @classmethod
    async def _list_cloud(cls, *, api_key: Optional[str] = None) -> "list[SandboxInfo]":
        if not cls._uses_fleet(api_key):
            from cua_sandbox.transport.cloud import cloud_list_vms

            vms = await cloud_list_vms(api_key=api_key)
            return [
                SandboxInfo(
                    location="cloud",
                    id=_refs.qualified("cloud", vm.get("name", "")),
                    name=vm.get("name", ""),
                    status=vm.get("status", "unknown"),
                    source="cloud",
                    os_type=vm.get("os_type") or vm.get("os"),
                    created_at=vm.get("created_at"),
                )
                for vm in vms
            ]

        from cua_sandbox import _autopool

        # Every claim in every pool this account can see: managed
        # (cua-auto-*) and explicit pools alike.
        claims = await _autopool.list_claims()
        return [cls._fleet_claim_info(claim) for claim in claims]

    @staticmethod
    def _fleet_claim_info(claim: Any) -> SandboxInfo:
        phase = (claim.phase or "").lower()
        status = {"bound": "running", "pending": "provisioning", "": "provisioning"}.get(
            phase, phase
        )
        return SandboxInfo(
            name=claim.name,
            status=status,
            source="fleet",
            created_at=claim.created_at,
            id=_refs.qualified("cloud", claim.name),
            location="cloud",
            provider_details={"pool": claim.pool, "namespace": claim.pool},
        )

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
            id=_refs.qualified("cloud", name),
            location="cloud",
            provider_details={"pool": name},
        )

    @classmethod
    async def get_info(
        cls,
        name: str,
        *,
        local: Optional[bool] = None,
        api_key: Optional[str] = None,
    ) -> "SandboxInfo":
        """Get metadata for a specific sandbox.

        Args:
            name: Sandbox ref (``local:<name>``, ``cloud:<name>``) or a name
                unique across locations.
            local: ``True``/``False`` narrow a bare name to local or cloud.
                ``None`` (default): the local sandbox ``name`` when one
                exists (and no cloud sandbox shares it), else the cloud.
            api_key: CUA API key for cloud.
        """
        name, local = await _lookup(name, local)
        if local:
            sandboxes = await cls._list_local()
            match = next((s for s in sandboxes if s.name == name), None)
            if match:
                return match
            # Fall back to state file
            from cua_sandbox import sandbox_state

            state = sandbox_state.load(name)
            if state:
                location = "cloud" if state.get("runtime_type") == "fleet" else "local"
                return SandboxInfo(
                    location=location,
                    id=_refs.qualified(location, name),
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
                location="cloud",
                id=_refs.qualified("cloud", vm.get("name", name)),
                name=vm.get("name", name),
                status=vm.get("status", "unknown"),
                source="cloud",
                os_type=vm.get("os_type") or vm.get("os"),
                created_at=vm.get("created_at"),
            )
        from cua_sandbox import sandbox_state

        state = sandbox_state.load(name) or {}
        if state.get("runtime_type") == "fleet" and state.get("pool_name"):
            from cua_sandbox import _autopool

            for claim in await _autopool.list_claims():
                if claim.name == name and claim.pool == state["pool_name"]:
                    return cls._fleet_claim_info(claim)
            raise ValueError(f"Fleet sandbox {name!r} not found in pool {state['pool_name']!r}")
        return cls._fleet_sandbox_info(await FleetCloudTransport.get_sandbox_info(name))

    @classmethod
    async def suspend(
        cls,
        name: str,
        *,
        local: Optional[bool] = None,
        api_key: Optional[str] = None,
    ) -> None:
        """Suspend the sandbox ``name``, keeping its state; :meth:`resume`
        continues it. Only that sandbox is affected.

        Local: the SDK suspends it on its backend (a container is paused or
        checkpointed, a QEMU VM snapshotted, a Lume VM stopped with its
        disk). Cloud: a Fleet sandbox cannot be suspended on its own, so this
        raises :class:`Unsupported`; keep it with ``keep_alive()`` or release
        it with :meth:`delete`. A legacy api.cua.ai VM (an API key) stops.

        Args:
            name: Sandbox name.
            local: ``None`` (default): the local sandbox ``name`` when one
                exists, else the cloud. ``True``/``False`` decide.
            api_key: Legacy CUA API key (api.cua.ai VMs).
        """
        name, local = await _lookup(name, local)
        if local:
            await cls._suspend_local(name)
            return
        if not cls._uses_fleet(api_key):
            from cua_sandbox.transport.cloud import cloud_vm_action

            await cloud_vm_action(name, "stop", api_key=api_key)
            return
        raise _fleet_lifecycle_unsupported(name, "suspend")

    @classmethod
    async def _suspend_local(cls, name: str) -> None:
        from cua_sandbox import sandbox_state
        from cua_sandbox.runtime.native import is_native_state

        state = sandbox_state.load(name)
        runtime_type = state.get("runtime_type") if state else None
        if is_native_state(state) or runtime_type in ("docker", "qemu-docker", "lume"):
            # The SDK finds the owning backend (container, QEMU, Lume) itself.
            from cua_sandbox._sdk import local_runtime

            handle = await local_runtime().sandboxes().connect(name)
            await handle.suspend()
            if state is not None and not is_native_state(state):
                sandbox_state.update(name, status="suspended")
        elif runtime_type == "qemu-baremetal":
            from cua_sandbox.runtime.qemu import QEMUBaremetalRuntime

            rt = QEMUBaremetalRuntime(use_sdk=False)
            if state:
                rt.qmp_port = state.get("qmp_port", rt.qmp_port)
            await rt.suspend(name)
        else:
            raise ValueError(f"Cannot suspend sandbox '{name}': unknown runtime '{runtime_type}'")

    @classmethod
    async def resume(
        cls,
        name: str,
        *,
        local: Optional[bool] = None,
        api_key: Optional[str] = None,
    ) -> "Sandbox":
        """Resume the suspended sandbox ``name`` and return it connected.

        Local: the backend resumes it. Cloud: a running sandbox just
        reconnects (a Fleet sandbox is never suspended); one that is gone
        raises :class:`Unsupported`. A legacy api.cua.ai VM (an API key)
        starts.

        Args:
            name: Sandbox name.
            local: ``None`` (default): the local sandbox ``name`` when one
                exists, else the cloud.
            api_key: Legacy CUA API key (api.cua.ai VMs).

        Returns:
            A connected Sandbox ready to use.
        """
        name, local = await _lookup(name, local)
        if local:
            return await cls._resume_local(name)
        if not cls._uses_fleet(api_key):
            from cua_sandbox.transport.cloud import cloud_vm_action

            await cloud_vm_action(name, "run", api_key=api_key)
        else:
            try:
                return await cls._create(name=name, ephemeral=False, api_key=api_key)
            except Exception as error:  # noqa: BLE001 - re-raised typed
                raise _fleet_lifecycle_unsupported(name, "resume") from error
        # Connect to the now-running cloud sandbox.
        sb = await cls._create(name=name, ephemeral=False, api_key=api_key)
        return sb

    @classmethod
    async def _resume_local(cls, name: str) -> "Sandbox":
        from cua_sandbox import sandbox_state
        from cua_sandbox.runtime.native import is_native_state

        state = sandbox_state.load(name)
        if state is None:
            raise ValueError(f"No local sandbox named '{name}' found in state files.")
        runtime_type = state.get("runtime_type")
        if is_native_state(state) or runtime_type in ("docker", "qemu-docker", "lume"):
            from cua_sandbox._sdk import local_runtime

            handle = await local_runtime().sandboxes().connect(name)
            await handle.resume()
            if not is_native_state(state):
                sandbox_state.update(name, status="running")
            sb = cls(
                await _local_state_transport(name, sandbox_state.load(name) or state),
                name=name,
                _ephemeral=False,
            )
            await sb._connect()
            return sb
        if runtime_type == "qemu-baremetal":
            from cua_sandbox.runtime.qemu import QEMUBaremetalRuntime

            image = Image.from_dict(state["image"])
            rt = QEMUBaremetalRuntime(
                api_port=state.get("api_port", 8000),
                vnc_display=state.get("vnc_display", 0),
                memory_mb=state.get("memory_mb", 4096),
                cpu_count=state.get("cpu_count", 2),
                arch=state.get("arch", "x86_64"),
                qmp_port=state.get("qmp_port", 4444),
                use_sdk=False,
            )
            rt_info = await rt.resume(image, name)
            transport = _env_transport(rt_info, environment=state.get("os_type"))
            sb = cls(transport, name=name, _ephemeral=False)
            await sb._connect()
            return sb
        raise ValueError(f"Cannot resume sandbox '{name}': unknown runtime_type '{runtime_type}'")

    @classmethod
    async def restart(
        cls,
        name: str,
        *,
        local: Optional[bool] = None,
        api_key: Optional[str] = None,
    ) -> "Sandbox":
        """Restart the sandbox ``name`` (suspend then resume) and return it
        connected. Local only (see :meth:`suspend`): a Fleet sandbox raises
        :class:`Unsupported`.

        Args:
            name: Sandbox name.
            local: ``None`` (default): the local sandbox ``name`` when one
                exists, else the cloud.
            api_key: Legacy CUA API key (api.cua.ai VMs).

        Returns:
            A connected Sandbox ready to use.
        """
        name, local = await _lookup(name, local)
        if local:
            await cls._suspend_local(name)
            return await cls._resume_local(name)
        if not cls._uses_fleet(api_key):
            from cua_sandbox.transport.cloud import cloud_vm_action

            await cloud_vm_action(name, "restart", api_key=api_key)
        else:
            raise _fleet_lifecycle_unsupported(name, "restart")
        sb = await cls._create(name=name, ephemeral=False, api_key=api_key)
        return sb

    @classmethod
    async def delete(
        cls,
        name: str,
        *,
        local: Optional[bool] = None,
        api_key: Optional[str] = None,
    ) -> None:
        """Permanently delete a sandbox.

        For local sandboxes, stops the VM and removes the state file.
        For cloud sandboxes, calls DELETE /v1/vms/{name}.

        Args:
            name: Sandbox name.
            local: If True, delete a local sandbox. ``None`` (default): the
                local sandbox ``name`` when one exists, else the cloud.
            api_key: CUA API key for cloud.
        """
        name, local = await _lookup(name, local)
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
        from cua_sandbox.runtime.native import is_native_state

        state = sandbox_state.load(name)
        runtime_type = state.get("runtime_type") if state else None
        if is_native_state(state) or runtime_type in ("docker", "qemu-docker", "lume"):
            from cua_sandbox._sdk import local_runtime

            # Deletes the instance (container, VM, Lume clone) and its state.
            await local_runtime().sandboxes().delete(name)
        elif runtime_type == "qemu-baremetal":
            from cua_sandbox.runtime.qemu import QEMUBaremetalRuntime

            await QEMUBaremetalRuntime(use_sdk=False).stop(name)  # stop() deletes the state file
            return
        elif runtime_type == "androidemulator":
            from cua_sandbox.runtime.android_emulator import AndroidEmulatorRuntime

            await AndroidEmulatorRuntime().stop(name)
        elif runtime_type is not None:
            raise ValueError(f"Cannot delete sandbox '{name}': unknown runtime '{runtime_type}'")
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
        url: Optional[str] = None,
        token: Optional[str] = None,
        api_key: Optional[str] = None,
        container_name: Optional[str] = None,
        image: Optional[Image] = None,
        runtime: Optional["Runtime | FleetRuntime"] = None,
        name: Optional[str] = None,
        pool: Optional[str] = None,
        ephemeral: Optional[bool] = None,
        cpu: Optional[int] = None,
        memory_mb: Optional[int] = None,
        disk_gb: Optional[int] = None,
        region: str = "us-east-1",
        time_to_start: Optional[float] = None,
        request_timeout: Optional[float] = None,
        server_port: Optional[int] = None,
        telemetry_enabled: bool = True,
        warm: Optional[bool] = None,
        max_pool_size: Optional[int] = None,
        claim_ttl: Any = None,
        progress: Optional[Callable[[Any], Any]] = None,
        command: Optional[list[str]] = None,
        sidecars: Optional[list] = None,
        engine: Optional[str] = None,
        network: Optional[str] = None,
        env: Optional[dict[str, str]] = None,
        services: Optional[dict[str, int]] = None,
        wait_for: Optional[list[Probe]] = None,
        hint: Optional[str] = None,
    ) -> "Sandbox":
        """Internal factory that validates server_port before selecting a transport.

        ``runtime`` is a legacy local :class:`Runtime` object; ``engine`` the
        engine name the placement resolved (``gvisor``, ``qemu``, ...), with
        ``image.kind`` already set to the resolved kind. ``hint`` is added to
        a missing-credentials error when the cloud came from a user default.
        """
        validate_server_port(server_port)
        auto_adapter = runtime is None

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
        if image and image.kind is None and local:
            # The one resolver (native cua_image): short refs are docker.io,
            # containerDisks run on QEMU, rootfs images as containers.
            from cua_sandbox.image import resolve_image_kind

            image = await asyncio.to_thread(resolve_image_kind, image)

        # Direct connection to a cua-spacesd by URL — no provider involved.
        direct_url = url or http_url
        if direct_url and not image:
            transport = EnvTransport(url=direct_url, token=token or api_key)
            sb = cls(
                transport,
                name=name or direct_url,
                _ephemeral=False,
                _telemetry_enabled=telemetry_enabled,
            )
            sb._direct_url = direct_url
            await sb._connect()
            _record_sandbox_create(sb, image=None, local=False, ephemeral=False, t_start=_t_start)
            return sb

        # Local connect by name — read state file
        if name and not image and local and not ws_url:
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
                transport = await _local_state_transport(name, state)
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
            runtime = _auto_runtime(
                image, cpu=cpu, memory_mb=memory_mb, server_port=server_port, engine=engine
            )
        if image and not runtime and not local:
            # image without runtime and not local → cloud creation
            _reject_cloud_network(network, False)
            if not any([ws_url, http_url]) and cls._uses_fleet(api_key):
                # Claimed from the account's managed pool for this image.
                _check_managed_options(
                    replicas=1,
                    claim_spec=None,
                    disk_gb=disk_gb,
                    region=region,
                    request_timeout=request_timeout,
                )
                sb = await _acquire_managed(
                    image,
                    name=name,
                    service=ENV_SERVICE,
                    cpu=cpu,
                    memory_mb=memory_mb,
                    server_port=server_port,
                    time_to_start=time_to_start,
                    warm=warm,
                    max_pool_size=max_pool_size,
                    claim_ttl=claim_ttl,
                    progress=progress,
                    telemetry_enabled=telemetry_enabled,
                    ephemeral=bool(ephemeral),
                    fleet_runtime=engine,
                    command=command,
                    sidecars=sidecars,
                    env=env,
                    services=services,
                    wait_for=wait_for,
                    hint=hint,
                )
                if not ephemeral:
                    await _save_fleet_claim_or_close(
                        sb, sb.claim_name, sb.pool_name, **sb._claim_handle.state_fields()
                    )
                return sb
            if not any([ws_url, http_url]):
                if command or env or services or wait_for or sidecars:
                    raise NotImplementedError(
                        "command, env, services, sidecars and wait_for need a cloud sandbox (Fleet "
                        "credentials: `cua auth login` or CUA_CLIENT_ID/CUA_CLIENT_SECRET) "
                        "or local=True"
                    )
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
            runtime = _auto_runtime(image, cpu=cpu, memory_mb=memory_mb, server_port=server_port)
        if image and runtime:
            sb_name = name or _random_name()
            # Forward the sizing knobs the caller gave Sandbox.create/ephemeral so a
            # local VM honours cpu= and memory_mb= the same way a Fleet pool does.
            start_opts: dict = {}
            if cpu is not None:
                start_opts["cpu_count"] = cpu
            if memory_mb is not None:
                start_opts["memory_mb"] = memory_mb
            if disk_gb is not None:
                start_opts["disk_size_gb"] = disk_gb
            if command or env or services or wait_for or sidecars:
                from cua_sandbox.runtime.native import NativeRuntime

                if not isinstance(runtime, NativeRuntime):
                    raise NotImplementedError(
                        f"{type(runtime).__name__} does not take command, env, services, "
                        "sidecars or wait_for; use the default local runtime (containers, "
                        "QEMU, Lume)"
                    )
                start_opts.update(
                    command=list(command) if command else None,
                    env=dict(env or {}),
                    services=dict(services or {}),
                    wait_for=list(wait_for or []),
                    sidecars=list(sidecars or []),
                )
            if auto_adapter and local and _sdk_backed(runtime):
                # The adapter was picked from the placement: the SDK gets the
                # kind and engine too (and checks them against the image).
                if image.kind:
                    start_opts["kind"] = image.kind
                if engine:
                    start_opts["runtime"] = engine
            if network == "none":
                _require_network_none_support(runtime)
                start_opts["network"] = "none"
            rt_info = await runtime.start(image, sb_name, ephemeral=bool(ephemeral), **start_opts)
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
            elif rt_info.native is None and not rt_info.api_port and rt_info.vnc_port:
                # VNC-only: a VM with no guest agent port at all.
                from cua_sandbox.transport.vnc import VNCTransport

                transport = VNCTransport(
                    host=rt_info.host,
                    port=rt_info.vnc_port,
                    environment=rt_info.environment or image.os_type,
                )
            elif rt_info.native is None and rt_info.qmp_port and rt_info.environment:
                # QMP-driven VM (use_qmp_transport / Android-x86): agentless.
                from cua_sandbox.transport.qmp import QMPTransport

                transport = QMPTransport(
                    qmp_host=rt_info.host,
                    qmp_port=rt_info.qmp_port,
                    environment=rt_info.environment or image.os_type,
                )
            else:
                # cua-spacesd when the image has it; QMP/VNC otherwise.
                transport = _env_transport(
                    rt_info, environment=rt_info.environment or image.os_type, token=token
                )
        else:
            if ws_url:
                _make_transport(ws_url=ws_url)  # raises: the WebSocket protocol is gone
            if name and cls._uses_fleet(api_key):
                from cua_sandbox import sandbox_state

                state = sandbox_state.load(name)
                pool_name = state.get("pool_name") if state else None
                osworld = bool(state) and state.get("agent_type") == "osworld"
                transport_cls = OSWorldFleetCloudTransport if osworld else FleetCloudTransport
                transport = transport_cls(
                    image=None,
                    name=name,
                    pool_name=pool_name,
                    cpu=cpu,
                    memory_mb=memory_mb,
                    disk_gb=disk_gb,
                    region=region,
                    **({"server_port": OSWORLD_SERVER_PORT} if osworld else {}),
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
        # Write persistent state for local (non-ephemeral) sandboxes. Runtimes
        # on the cua SDK persisted it themselves, in the same format.
        if not ephemeral and rt_info and local and rt_info.native is None:
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
        sb._image_info_fallback = getattr(image, "_resolved", None) if image else None
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
    """Transport for a non-Fleet, non-local target.

    ``http_url`` names a cua-spacesd; ``ws_url`` (the computer-server
    WebSocket protocol) is gone; with neither, the legacy API-key cloud shim
    is returned (it explains the move to Fleet on connect).
    """
    if ws_url:
        raise ValueError(
            "ws_url= spoke the computer-server WebSocket protocol, which was removed. "
            "Connect to the sandbox's cua-spacesd instead: "
            "Sandbox.connect(url='http://host:3211', token=...)"
        )
    if http_url:
        return EnvTransport(url=http_url, token=api_key)
    return CloudTransport(
        name=name,
        api_key=api_key,
        image=image,
        cpu=cpu,
        memory_mb=memory_mb,
        disk_gb=disk_gb,
        region=region,
    )


def _fallback_transport(
    host: str,
    *,
    qmp_port: Optional[int] = None,
    vnc_port: Optional[int] = None,
    environment: Optional[str] = None,
) -> Optional[Transport]:
    """An agentless transport (QMP, else VNC) for sandboxes without spacesd."""
    if qmp_port:
        from cua_sandbox.transport.qmp import QMPTransport

        return QMPTransport(qmp_host=host, qmp_port=qmp_port, environment=environment or "linux")
    if vnc_port:
        from cua_sandbox.transport.vnc import VNCTransport

        return VNCTransport(host=host, port=vnc_port, environment=environment or "linux")
    return None


def _env_transport(
    rt_info: "RuntimeInfo",
    *,
    environment: Optional[str] = None,
    token: Optional[str] = None,
) -> EnvTransport:
    """EnvTransport for a started local runtime (SDK-backed or legacy)."""
    fallback = _fallback_transport(
        rt_info.host,
        qmp_port=rt_info.qmp_port,
        vnc_port=rt_info.vnc_port,
        environment=environment,
    )
    ready_timeout = rt_info.env_ready_timeout or (120.0 if fallback is not None else 60.0)
    vnc_url = f"vnc://{rt_info.host}:{rt_info.vnc_port}" if rt_info.vnc_port else None
    handle = rt_info.native
    if handle is not None:
        from cua_sandbox.transport.agentless import (
            AgentlessTransport,
            supports_agentless,
        )

        if supports_agentless(handle):
            # A local Lume sandbox: once cua-spacesd proves absent (the macOS
            # images), shell via SSH and screen via VNC, the path `cua sb exec`
            # and `cua sb screenshot` use. Lume's VNC needs a password the
            # plain VNCTransport does not have.
            fallback = AgentlessTransport(handle, environment=environment)
        direct = env_url(rt_info.host, rt_info.api_port) if rt_info.api_port else None
        env_token = token or rt_info.env_token
        # The SDK reports no `env` service for an image without cua-spacesd:
        # say so at once instead of probing a port nothing listens on.
        no_env = rt_info.guest_server_port is None

        async def open_env() -> Any:
            from cua_sandbox._sdk import connect_url, millis

            if no_env:
                raise SpacesdNotAvailable(
                    "this sandbox's image does not run cua-spacesd; use its named "
                    "services (sb.services.request) or sb.tunnel.forward(port)"
                )

            if direct and env_token:
                # A handle re-attached from state carries no token; dial the
                # published env port with the recorded one.
                return await (await connect_url(direct, env_token)).spacesd(millis(15))
            return await handle.spacesd(millis(15))

        return EnvTransport(
            env_factory=open_env,
            environment=environment,
            fallback=fallback,
            ready_timeout=ready_timeout,
            native_sandbox=handle,
            vnc_url=vnc_url,
        )
    return EnvTransport(
        url=env_url(rt_info.host, rt_info.api_port),
        token=token or rt_info.env_token,
        environment=environment,
        fallback=fallback,
        ready_timeout=ready_timeout,
        vnc_url=vnc_url,
    )


async def _local_state_transport(name: str, state: dict) -> EnvTransport:
    """EnvTransport for a local sandbox recorded in ``~/.cua/sandboxes``."""
    from cua_sandbox.runtime.base import RuntimeInfo
    from cua_sandbox.runtime.native import is_native_state, runtime_info_from_state

    if is_native_state(state):
        from cua_sandbox._sdk import local_runtime

        handle = await local_runtime().sandboxes().connect(name)
        rt_info = runtime_info_from_state(name, state, handle=handle)
    else:
        rt_info = RuntimeInfo(
            host=state.get("host") or "localhost",
            api_port=int(state.get("api_port") or 0),
            vnc_port=state.get("vnc_port"),
            qmp_port=state.get("qmp_port"),
            name=name,
            environment=state.get("os_type"),
        )
    return _env_transport(rt_info, environment=state.get("os_type"))


@asynccontextmanager
async def sandbox(
    *,
    on: Optional[str] = None,
    local: Optional[bool] = None,
    kind: Optional[str] = None,
    ws_url: Optional[str] = None,
    http_url: Optional[str] = None,
    url: Optional[str] = None,
    token: Optional[str] = None,
    api_key: Optional[str] = None,
    container_name: Optional[str] = None,
    image: Optional[Image] = None,
    runtime: "Runtime | str | None" = None,
    name: Optional[str] = None,
    ephemeral: Optional[bool] = None,
    cpu: Optional[int] = None,
    memory_mb: Optional[int] = None,
    disk_gb: Optional[int] = None,
    region: str = "us-east-1",
    warm: Optional[bool] = None,
    max_pool_size: Optional[int] = None,
    claim_ttl: Any = None,
    progress: Optional[Callable[[Any], Any]] = None,
) -> AsyncIterator[Sandbox]:
    """Async context manager for a sandboxed environment.

    ``on``/``local``, ``kind`` and ``runtime`` mean what they mean for
    :meth:`Sandbox.create`.

    .. deprecated::
        Prefer ``Sandbox.create()``, ``Sandbox.connect()``, or
        ``Sandbox.ephemeral()`` instead.
    """
    engine, hint = None, None
    if image is not None and not (url or http_url or ws_url):
        # A new sandbox: the same placement rules as Sandbox.create.
        on, local, runtime = _apply_on(on, local, runtime, cpu=cpu, memory_mb=memory_mb)
        place, image = _place_new(
            image,
            on=on,
            local=local,
            kind=kind,
            runtime=runtime,
            cloud=None,
            cloud_only=_cloud_only_args(
                warm=warm,
                max_pool_size=max_pool_size,
                claim_ttl=claim_ttl,
                api_key=api_key,
                region=region,
                progress=progress,
            ),
        )
        local, engine, runtime = place.local, place.runtime, place.legacy_runtime
        hint = place.cloud_default_hint()
    elif on is not None:
        local = _placement.resolve(on=on, local=local).local
    sb = await Sandbox._create(
        local=bool(local),
        ws_url=ws_url,
        http_url=http_url,
        url=url,
        token=token,
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
        warm=warm,
        max_pool_size=max_pool_size,
        claim_ttl=claim_ttl,
        progress=progress,
        engine=engine,
        hint=hint,
    )
    try:
        yield sb
    finally:
        if sb._ephemeral:
            await sb.destroy()
        else:
            await sb.disconnect()

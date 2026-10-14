"""Managed Fleet pools for ``Sandbox.create`` / ``Sandbox.ephemeral`` without ``pool=``.

This module is the single adapter between cua-sandbox and the cua SDK's
native auto pool manager (``cua_fleet::autopool::PoolManager``, reached
through the ``cua`` binding). Everything else in the package calls only the
module-level API:

* :func:`config` builds an :class:`AutoPoolConfig` (defaults + env overrides).
* :func:`acquire` returns a connected :class:`~cua_sandbox.Sandbox` on the
  managed pool for an image spec, creating or reusing the pool.
* :func:`list_claims`, :func:`list_pools` and :func:`gc` back
  ``Sandbox.list()`` and pool maintenance.

The native manager implements the whole contract: one pool per tenant +
spec (``cua-auto-<base32(sha256(sub || spec_hash))[:16]>``, image tags
resolved to digests), KEDA autoscaling from zero, an existing pool's spec is
never rewritten, claims carry ``ttlSecondsAfterCreated = claim_ttl`` and a
heartbeat in the SDK runtime renews them while this process holds them, and
idle pools are garbage-collected (automatically at most once an hour per
machine, or on demand with :func:`gc`). The spec encoding helpers below
mirror it for inspection and tests.
"""

from __future__ import annotations

import asyncio
import base64
import hashlib
import json
import logging
import os
import re
from dataclasses import dataclass, field, replace
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import TYPE_CHECKING, Any, Callable, Mapping, Optional, Union

from cua_sandbox import _sdk
from cua_sandbox._paths import cua_home, patched_or
from cua_sandbox._sdk import ENV_SERVICE, SPACESD_PORT
from cua_sandbox.image import Image, cloud_registry_image
from cua_sandbox.pool import _ClaimHandle
from cua_sandbox.transport.fleet_cloud import (
    _NATIVE_POOL_ACCESS_DENIED,
    FleetCloudTransport,
    default_server_port,
    resolve_fleet_runtime,
)
from fleet_sdk import SdkError

if TYPE_CHECKING:
    from cua_sandbox.sandbox import Sandbox

logger = logging.getLogger(__name__)

MANAGED_PREFIX = "cua-auto-"
LEGACY_EPHEMERAL_PREFIX = "cua-eph-"
LABEL_MANAGED_BY = "cua.ai/managed-by"
MANAGED_BY = "cua-sdk"
LABEL_SPEC_HASH = "cua.ai/spec-hash"
LABEL_LAST_USED = "cua.ai/last-used"

#: Floor of a claim's bind deadline (``cua_fleet::autopool::MIN_BIND_DEADLINE_SECS``):
#: live cold binds took 80 to 530 s.
MIN_BIND_DEADLINE = 900
_NAME_HASH_CHARS = 16
_MAX_NAME_ATTEMPTS = 8
_SPEC_HASH_LABEL_CHARS = 32
_LIST_CONCURRENCY = 8
_PROGRESS_TICK = 15.0
_MAX_PROGRESS_TICKS = 240  # 1 hour of 15 s ticks
_MIN_CLAIM_TTL = 30
_MAX_CLAIM_TTL = 7 * 86400
_COLD_START_NOTE = "the first start of an image can take a few minutes"
_STARTING_MESSAGE = "Starting a cloud sandbox (first start of an image can take a few minutes)"

#: Where the native manager keeps its pool-name cache and GC lock (patched
#: by tests).
CUA_DIR = cua_home()
_CUA_DIR_DEFAULT = CUA_DIR


def cua_dir() -> Path:
    """``$CUA_HOME`` now (or the patched ``CUA_DIR``)."""
    return patched_or(CUA_DIR, _CUA_DIR_DEFAULT)


Duration = Union[int, float, timedelta]


class AutoPoolError(RuntimeError):
    """A managed Fleet pool could not provide a sandbox."""


# ── Configuration ────────────────────────────────────────────────────────


def _parse_duration(value: str, variable: str) -> int:
    match = re.fullmatch(r"\s*(\d+)\s*([smhd]?)\s*", value.lower())
    if match is None:
        raise ValueError(f"{variable} must be a duration like 900, 90s, 15m, 1h or 7d")
    amount, unit = int(match.group(1)), match.group(2) or "s"
    return amount * {"s": 1, "m": 60, "h": 3600, "d": 86400}[unit]


def _seconds(value: Duration, field_name: str) -> int:
    if isinstance(value, timedelta):
        seconds = value.total_seconds()
    elif isinstance(value, bool) or not isinstance(value, (int, float)):
        raise TypeError(f"{field_name} must be seconds (int/float) or a timedelta")
    else:
        seconds = float(value)
    return int(round(seconds))


@dataclass(frozen=True)
class AutoPoolConfig:
    """Tuning for managed pools. Build it with :func:`config`."""

    max_pool_size: int = 10
    claim_ttl: int = 15 * 60
    warm: bool = False
    #: Whether ``warm`` was chosen (argument or ``CUA_FLEET_WARM``); unset,
    #: the SDK makes the canonical images warm and others cold.
    warm_set: bool = False
    bind_deadline: int = MIN_BIND_DEADLINE
    pool_ttl: int = 7 * 86400
    idle_gc: int = 30 * 60
    auto_gc: bool = True

    @property
    def initial_pool_size(self) -> int:
        return 1 if self.warm else 0

    def validate(self) -> "AutoPoolConfig":
        if (
            isinstance(self.max_pool_size, bool)
            or not isinstance(self.max_pool_size, int)
            or self.max_pool_size < 1
        ):
            raise ValueError("max_pool_size must be a positive integer")
        if not _MIN_CLAIM_TTL <= self.claim_ttl <= _MAX_CLAIM_TTL:
            raise ValueError(
                f"claim_ttl must be between {_MIN_CLAIM_TTL} seconds and 7 days "
                f"(got {self.claim_ttl}s); use sb.keep_alive(minutes=...) to hold a "
                "sandbox longer"
            )
        if self.bind_deadline < 1 or self.pool_ttl < 3600 or self.idle_gc < 0:
            raise ValueError("bind_deadline, pool_ttl and idle_gc must be positive")
        return self


def config(
    *,
    warm: Optional[bool] = None,
    max_pool_size: Optional[int] = None,
    claim_ttl: Optional[Duration] = None,
    environ: Optional[Mapping[str, str]] = None,
) -> AutoPoolConfig:
    """Defaults, then ``CUA_FLEET_*`` env overrides, then explicit kwargs."""
    env = os.environ if environ is None else environ
    cfg = AutoPoolConfig()
    if env.get("CUA_FLEET_MAX_POOL_SIZE", "").strip():
        try:
            cfg = replace(cfg, max_pool_size=int(env["CUA_FLEET_MAX_POOL_SIZE"]))
        except ValueError as error:
            raise ValueError("CUA_FLEET_MAX_POOL_SIZE must be an integer") from error
    if env.get("CUA_FLEET_CLAIM_TTL", "").strip():
        cfg = replace(
            cfg, claim_ttl=_parse_duration(env["CUA_FLEET_CLAIM_TTL"], "CUA_FLEET_CLAIM_TTL")
        )
    idle = env.get("CUA_FLEET_POOL_IDLE_GC", "").strip().lower()
    if idle in ("off", "false", "never", "0"):
        cfg = replace(cfg, auto_gc=False)
    elif idle:
        cfg = replace(cfg, idle_gc=_parse_duration(idle, "CUA_FLEET_POOL_IDLE_GC"))
    if env.get("CUA_FLEET_WARM", "").strip():
        cfg = replace(
            cfg,
            warm=env["CUA_FLEET_WARM"].strip().lower() in ("1", "true", "yes"),
            warm_set=True,
        )
    if warm is not None:
        cfg = replace(cfg, warm=bool(warm), warm_set=True)
    if max_pool_size is not None:
        cfg = replace(cfg, max_pool_size=max_pool_size)
    if claim_ttl is not None:
        cfg = replace(cfg, claim_ttl=_seconds(claim_ttl, "claim_ttl"))
    return cfg.validate()


# ── Spec key and naming ──────────────────────────────────────────────────


@dataclass(frozen=True)
class PoolSpecKey:
    """Everything that makes two sandboxes interchangeable.

    Mirrors ``cua_fleet::autopool::PoolSpecKey`` field for field so both
    implementations hash (and therefore name) a spec identically.
    """

    image: str
    runtime: str = "kubevirt"
    efi: bool = False
    cpu: Optional[int] = None
    memory_mb: Optional[int] = None
    services: tuple[tuple[str, int], ...] = (("env", 3211),)
    readiness_tcp_port: Optional[int] = None
    command: Optional[tuple[str, ...]] = None
    env: tuple[tuple[str, str], ...] = ()

    def canonical(self) -> bytes:
        """Versioned JSON array with a fixed field order (serde_json compact)."""
        fields: list = [
            "cua-autopool/v1",
            ["image", self.image.strip()],
            ["runtime", self.runtime],
            ["efi", self.efi],
            ["cpu", self.cpu],
            ["memory_mb", self.memory_mb],
            ["services", [[name, port] for name, port in sorted(self.services)]],
            ["readiness_tcp_port", self.readiness_tcp_port],
            ["command", list(self.command) if self.command is not None else None],
        ]
        # Only when set, so specs without env keep their key (and pool).
        if self.env:
            fields.append(["env", [[k, v] for k, v in sorted(self.env)]])
        return json.dumps(fields, separators=(",", ":"), ensure_ascii=False).encode()

    def spec_hash(self) -> str:
        return hashlib.sha256(self.canonical()).hexdigest()

    @property
    def label(self) -> str:
        return self.spec_hash()[:_SPEC_HASH_LABEL_CHARS]


#: sandbox-core's shape defaults, part of the key so both SDKs share pools.
DEFAULT_CPU = 2
DEFAULT_MEMORY_MB = 4096


def managed_services(image: Image, server_port: Optional[int] = None) -> dict[str, int]:
    """Services a managed pool publishes: always ``env`` (3211), plus
    ``server`` for a declared ``server_port`` and ``port-<n>`` per exposed
    port (sandbox-core's ``all_services`` rule)."""
    services = {"env": SPACESD_PORT}
    if server_port is not None:
        services["server"] = server_port
    for port in image._ports:
        if port not in services.values():
            services[f"port-{port}"] = port
    return services


def spec_key(
    image: Image,
    *,
    cpu: Optional[int] = None,
    memory_mb: Optional[int] = None,
    server_port: Optional[int] = None,
    runtime: str = "kubevirt",
) -> PoolSpecKey:
    FleetCloudTransport._validate_image(image)
    reference = cloud_registry_image(image)
    assert reference is not None
    return PoolSpecKey(
        image=reference,
        runtime=runtime,
        efi=image.os_type == "windows",
        cpu=cpu if cpu is not None else DEFAULT_CPU,
        memory_mb=memory_mb if memory_mb is not None else DEFAULT_MEMORY_MB,
        services=tuple(sorted(managed_services(image, server_port).items())),
        readiness_tcp_port=server_port,
    )


def _runtime_name(kind: Any) -> str:
    """``fleet_sdk.RuntimeKind`` -> its serde name (``kubevirt``/``gvisor``)."""
    return str(getattr(kind, "name", kind)).lower()


def pool_name_for(tenant: str, spec_hash: str, prefix: str = MANAGED_PREFIX) -> str:
    digest = hashlib.sha256(tenant.encode() + b"\0" + spec_hash.encode()).digest()
    encoded = base64.b32encode(digest).decode().lower().rstrip("=")
    return prefix + encoded[:_NAME_HASH_CHARS]


def candidate_names(base: str) -> list[str]:
    return [base] + [f"{base}-{index}" for index in range(2, _MAX_NAME_ATTEMPTS + 1)]


def is_managed_pool_name(name: str, prefix: str = MANAGED_PREFIX) -> bool:
    return name.startswith((prefix, MANAGED_PREFIX, LEGACY_EPHEMERAL_PREFIX))


# ── Small helpers ────────────────────────────────────────────────────────


def _client_factory() -> Any:
    # Resolved at call time so tests (and FakeFleet) patch one seam:
    # cua_sandbox.pool._FleetClient.
    from cua_sandbox import pool

    return pool._FleetClient()


def _status(error: BaseException) -> Optional[int]:
    status = getattr(error, "status", None)
    return status if isinstance(status, int) else None


def _is_access_denied(error: BaseException) -> bool:
    return (bool(_NATIVE_POOL_ACCESS_DENIED) and isinstance(error, _NATIVE_POOL_ACCESS_DENIED)) or (
        isinstance(error, SdkError) and _status(error) == 403
    )


def _is_not_found(error: BaseException) -> bool:
    return isinstance(error, LookupError) or _status(error) == 404


def _labels(resource: Any) -> dict[str, str]:
    metadata = getattr(resource, "metadata", None)
    return dict(getattr(metadata, "labels", None) or {})


def _parse_time(value: Optional[str]) -> Optional[datetime]:
    if not value:
        return None
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None
    return parsed if parsed.tzinfo else parsed.replace(tzinfo=timezone.utc)


def _now() -> datetime:
    return datetime.now(timezone.utc)


def _rfc3339(moment: datetime) -> str:
    return moment.astimezone(timezone.utc).isoformat().replace("+00:00", "Z")


def _claim_phase(claim: Any) -> Optional[str]:
    status = getattr(claim, "status", None)
    return getattr(status, "phase", None) if status is not None else None


def _claim_reason(claim: Any) -> str:
    status = getattr(claim, "status", None)
    for condition in reversed(list(getattr(status, "conditions", None) or [])):
        reason = getattr(condition, "reason", None)
        message = getattr(condition, "message", None)
        if reason or message:
            return ": ".join(part for part in (reason, message) if part)
    return ""


# ── Progress ─────────────────────────────────────────────────────────────


@dataclass(frozen=True)
class AcquireProgress:
    """One step of starting a cloud sandbox, passed to ``progress=``
    callbacks.

    ``stage`` is ``"provisioning"`` (capacity is being found or created),
    ``"starting"`` (periodic while the sandbox boots) or ``"ready"``.
    ``claim`` is the sandbox id once known; ``pool`` is always ``None``
    (kept for compatibility; see ``provider_details``).
    """

    stage: str
    pool: Optional[str]
    message: str
    claim: Optional[str] = None
    elapsed: float = 0.0


ProgressCallback = Callable[[AcquireProgress], Any]


def _emit(progress: Optional[ProgressCallback], event: AcquireProgress) -> None:
    if progress is None:
        return
    try:
        progress(event)
    except Exception:  # noqa: BLE001 - a broken callback must not fail the acquire
        logger.exception("progress callback raised")


# ── The managed claim handle ─────────────────────────────────────────────


class ManagedClaimHandle(_ClaimHandle):
    """A claim on a managed pool, held by the SDK runtime.

    The runtime's heartbeat renews the claim while the handle is held;
    :meth:`detach` stops it (the claim then lives until its shutdown time)
    and :meth:`release` deletes the claim (the pool stays for reuse).
    """

    managed = True

    def __init__(
        self,
        *,
        namespace: str,
        name: str,
        service: str,
        native: Any,
        claim_ttl: int,
        agent_type: Optional[str] = None,
    ) -> None:
        super().__init__(
            namespace=namespace,
            name=name,
            pool_name=namespace,
            service=service,
            agent_type=agent_type,
        )
        self._native = native
        self.claim_ttl = claim_ttl

    @property
    def heartbeat_running(self) -> bool:
        return self._native is not None

    def detach(self) -> None:
        """Stop renewing; the claim lives until its current shutdown time."""
        native, self._native = self._native, None
        if native is not None:
            try:
                native.detach()
            except Exception as error:  # noqa: BLE001 - the claim still expires by TTL
                logger.debug("detaching Fleet claim %s failed: %s", self.name, error)

    async def renew(self, shutdown_time: str) -> None:
        native = self._native
        if native is None:
            await super().renew(shutdown_time)
            return
        until = _parse_time(shutdown_time)
        seconds = max(1, int((until - _now()).total_seconds())) if until else self.claim_ttl
        # The runtime's heartbeat never renews to an earlier time than this.
        await native.keep_alive(seconds)

    async def release(self) -> None:
        native, self._native = self._native, None
        if native is None:
            await super().release()
            return
        try:
            await native.delete()
        except Exception as error:
            if not _sdk.is_not_found(error):
                raise

    def state_fields(self) -> dict[str, Any]:
        fields: dict[str, Any] = {"managed": True, "claim_ttl": self.claim_ttl}
        if self.agent_type:
            fields["agent_type"] = self.agent_type
        return fields


# ── Reports ──────────────────────────────────────────────────────────────


@dataclass
class ManagedPoolInfo:
    name: str
    spec_hash: Optional[str]
    managed: bool
    replicas: Optional[int]
    ready_replicas: Optional[int]
    claims: int
    last_used: Optional[datetime]
    created_at: Optional[str]


@dataclass
class ClaimInfo:
    name: str
    pool: str
    phase: Optional[str]
    managed: bool
    created_at: Optional[str]


@dataclass
class GcReport:
    pools_deleted: list[str] = field(default_factory=list)
    claims_deleted: list[str] = field(default_factory=list)
    namespaces_deleted: list[str] = field(default_factory=list)
    errors: list[str] = field(default_factory=list)


# ── Native manager access ────────────────────────────────────────────────


def _native_cua() -> Any:
    """The embedded SDK runtime that owns managed claims and heartbeats.

    One per process (cached by ``_sdk.runtime``): the SDK's own Tokio runtime
    keeps renewing claims under the sync facade too.
    """
    return _sdk.runtime(state_dir=_sdk._private_state_dir(), fleet=True, pool_home=cua_dir())


def _namespace_of(info: Any) -> Optional[str]:
    """The pool namespace from a Fleet sandbox's gateway endpoint."""
    for url in (getattr(info, "endpoints", None) or {}).values():
        _, sep, rest = url.partition("/api/svc/")
        if sep and rest:
            return rest.split("/", 1)[0]
    return None


# ── Module API (the adapter surface) ─────────────────────────────────────


async def acquire(
    image: Image,
    *,
    name: Optional[str] = None,
    cpu: Optional[int] = None,
    memory_mb: Optional[int] = None,
    server_port: Optional[int] = None,
    service: str = ENV_SERVICE,
    time_to_start: Optional[float] = None,
    cfg: Optional[AutoPoolConfig] = None,
    progress: Optional[ProgressCallback] = None,
    runtime: Optional[str] = None,
    command: Optional[list[str]] = None,
    env: Optional[dict[str, str]] = None,
    services: Optional[dict[str, int]] = None,
    wait_for: Optional[list] = None,
    sidecars: Optional[list] = None,
    hint: Optional[str] = None,
) -> "Sandbox":
    """Claim a sandbox for ``image`` from this account's managed pool.

    ``runtime`` is ``"kubevirt"`` or ``"gvisor"``; unset, it defaults from
    the image kind (``image.kind``, else the image itself: the cua SDK's
    single runtime/image rule). ``hint`` is appended when there are no
    cloud credentials (the cloud came from a user default). ``command``,
    ``env``, ``services`` and ``wait_for`` are part of the pool's template
    (and key); the SDK waits for the probes before returning.
    """
    from cua_sandbox import _placement
    from cua_sandbox.containers import has_build, image_build, sidecars_of

    cfg = cfg or config()
    FleetCloudTransport._validate_image(image)
    build = image_build(image)
    if image._secret is not None or build is not None:
        # The native core reads a private image with its secret, and a build's
        # output is a container rootfs: no anonymous pre-resolution here.
        runtime_name = runtime or ("gvisor" if has_build(image) else None)
    else:
        runtime_name = _runtime_name(resolve_fleet_runtime(runtime, image))
    reference = cloud_registry_image(image)
    assert reference is not None
    # Legacy OSWorld adapter: the disk runs the OSWorld Flask server, not
    # cua-spacesd, so the pool publishes it as ``server`` on 5000.
    server_port = default_server_port(image, server_port)
    if server_port is not None and service == ENV_SERVICE:
        # A declared server_port is the image's own daemon: wait for it.
        service = "server"
    native = _sdk.native()
    bind_budget = max(cfg.bind_deadline, int(time_to_start or 0), MIN_BIND_DEADLINE)
    options = native.SandboxCreateOptions(
        on="cloud",
        kind=image.kind,
        runtime=runtime_name,
        image=reference,
        name=name,
        os="windows" if image.os_type == "windows" else "linux",
        cpus=cpu if cpu is not None else DEFAULT_CPU,
        memory_mb=memory_mb if memory_mb is not None else DEFAULT_MEMORY_MB,
        services={
            **{k: v for k, v in managed_services(image, server_port).items() if k != "env"},
            **dict(services or {}),
        },
        ready_timeout_ms=_sdk.millis(bind_budget),
        command=list(command) if command else None,
        env=dict(env or {}),
        wait_for=[p.native() for p in (wait_for or [])],
        cloud=native.CloudOptions(
            # Unset lets the SDK decide: warm for the canonical images.
            warm=cfg.warm if cfg.warm_set else None,
            max_pool_size=cfg.max_pool_size,
            claim_ttl_seconds=cfg.claim_ttl,
            pool=None,
        ),
        sidecars=[c.native() for c in sidecars_of(sidecars)],
        registry_secret=image._secret.native() if image._secret is not None else None,
        build=build,
    )
    loop = asyncio.get_running_loop()
    started = loop.time()
    _emit(progress, AcquireProgress("provisioning", None, _STARTING_MESSAGE))
    if build is not None:
        _emit(
            progress,
            AcquireProgress(
                "provisioning",
                None,
                "building the image's layers in the cloud (an identical image is reused)",
            ),
        )

    async def ticker() -> None:
        for _ in range(_MAX_PROGRESS_TICKS):
            await asyncio.sleep(_PROGRESS_TICK)
            elapsed = loop.time() - started
            _emit(
                progress,
                AcquireProgress(
                    "starting",
                    None,
                    f"waiting for the sandbox to start ({elapsed:.0f}s)",
                    elapsed=elapsed,
                ),
            )

    ticks = asyncio.ensure_future(ticker()) if progress is not None else None
    try:
        handle_native = await _native_cua().sandboxes().create(options)
    except native.CuaError.ProviderNotConfigured as error:
        # No cloud credentials: say so (and how to switch back to local when
        # the cloud came from a default), not the cold-start hint.
        raise ValueError(f"{error}; {hint}" if hint else str(error)) from error
    except (native.CuaError.InvalidPlacement, native.CuaError.InvalidArgument) as error:
        raise _placement.translate(error) from None
    except Exception as error:
        elapsed = loop.time() - started
        advice = (
            f"no cloud sandbox for {reference} after {elapsed:.0f}s ({_COLD_START_NOTE}). "
            f"If this persists, {cfg.max_pool_size} sandboxes of this image may already be "
            "running (cloud=CloudOptions(max_pool_size=...)) or the image may not start. "
            "Pass time_to_start= to wait longer or cloud=CloudOptions(warm=True) to keep one "
            "ready."
        )
        raise AutoPoolError(f"{advice} Details: {error}") from error
    finally:
        if ticks is not None:
            ticks.cancel()
    info = handle_native.info()
    namespace = dict(info.provider_details).get("namespace") or _namespace_of(info)
    if namespace is None:
        handle_native.detach()
        raise AutoPoolError(f"cloud sandbox {info.name} reported no endpoint")
    handle = ManagedClaimHandle(
        namespace=namespace,
        name=info.name,
        service=service,
        native=handle_native,
        claim_ttl=cfg.claim_ttl,
        agent_type=image._agent_type,
    )
    if services or wait_for:
        # The SDK already waited for the probes; the claim is the readiness.
        service = ENV_SERVICE
    try:
        sandbox = await handle.wait(service=service, time_to_start=time_to_start)
        # Services, forwards and public URLs use the handle that holds the
        # claim (and its heartbeat).
        if getattr(sandbox._transport, "_native_fleet_sandbox", "unset") is None:
            sandbox._transport._native_fleet_sandbox = handle_native
    except BaseException:
        try:
            await handle.release()
        except Exception:  # noqa: BLE001 - the TTL reaps it
            pass
        raise
    elapsed = loop.time() - started
    _emit(
        progress,
        AcquireProgress(
            "ready",
            None,
            f"sandbox {info.name} ready in {elapsed:.0f}s",
            claim=info.name,
            elapsed=elapsed,
        ),
    )
    return sandbox


async def list_claims(cfg: Optional[AutoPoolConfig] = None) -> list[ClaimInfo]:
    """Claims in every pool this account can see (managed and explicit)."""
    client = _client_factory()
    try:
        names = [ns.name for ns in await client.list_namespaces()]
        gate = asyncio.Semaphore(_LIST_CONCURRENCY)

        async def claims_in(namespace: str) -> list[ClaimInfo]:
            async with gate:
                try:
                    claims = await client.list_claims(namespace)
                except Exception as error:  # noqa: BLE001
                    if _is_not_found(error) or _is_access_denied(error):
                        return []
                    raise
            managed_pool = is_managed_pool_name(namespace)
            return [
                ClaimInfo(
                    name=claim.metadata.name,
                    pool=namespace,
                    phase=_claim_phase(claim),
                    managed=managed_pool or _labels(claim).get(LABEL_MANAGED_BY) == MANAGED_BY,
                    created_at=claim.metadata.creation_timestamp,
                )
                for claim in claims
            ]

        results = await asyncio.gather(*(claims_in(name) for name in names))
        return [claim for group in results for claim in group]
    finally:
        await client.close()


def _pools_api() -> Any:
    return _native_cua().fleet().pools()


async def list_pools(cfg: Optional[AutoPoolConfig] = None) -> list[ManagedPoolInfo]:
    """This account's managed pools (``cua-auto-*`` and legacy ``cua-eph-*``)."""
    return [
        ManagedPoolInfo(
            name=p.name,
            spec_hash=p.spec_hash,
            managed=p.managed,
            replicas=p.replicas,
            ready_replicas=p.ready_replicas,
            claims=p.claims,
            last_used=(
                datetime.fromtimestamp(p.last_used_unix, timezone.utc)
                if p.last_used_unix is not None
                else None
            ),
            created_at=(
                datetime.fromtimestamp(p.created_unix, timezone.utc).isoformat()
                if p.created_unix is not None
                else None
            ),
        )
        for p in await _pools_api().list()
    ]


async def gc(
    idle_after: Optional[Duration] = None, cfg: Optional[AutoPoolConfig] = None
) -> GcReport:
    """Delete idle managed pools and stuck managed claims now."""
    idle = (cfg or config()).idle_gc if idle_after is None else _seconds(idle_after, "idle_after")
    report = await _pools_api().gc(idle)
    return GcReport(
        pools_deleted=list(report.deleted_pools),
        claims_deleted=list(report.deleted_claims),
        namespaces_deleted=list(report.deleted_namespaces),
        errors=list(report.errors),
    )


async def gc_pools(names: list[str], idle_after: Duration = 0) -> GcReport:
    """Delete the named managed pools once they have no claims and have been
    idle for ``idle_after`` (default: now). Pools with live claims are kept.
    Tests use this to remove exactly the pools they created."""
    report = await _pools_api().gc_pools(list(names), _seconds(idle_after, "idle_after"))
    return GcReport(
        pools_deleted=list(report.deleted_pools),
        claims_deleted=list(report.deleted_claims),
        namespaces_deleted=list(report.deleted_namespaces),
        errors=list(report.errors),
    )


def stop_all_heartbeats() -> None:
    """Kept for compatibility: heartbeats live in the SDK runtime and stop
    with each sandbox's ``close``/``disconnect``."""

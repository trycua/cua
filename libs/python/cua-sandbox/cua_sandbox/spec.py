"""One sandbox model for pools and ``Sandbox.create``.

:class:`SandboxSpec` is what a sandbox runs (image, process, services,
readiness, resources, sidecars, registry secret, process mode, claim
secrets) and :class:`PoolOptions` how a pool keeps capacity for it (warm
floor, size, idle TTL, TTL policy, claim TTL, runtime). Both convert to the
cua SDK's native records, and ``Pool.apply(name, spec, options)`` writes a
pool through the SDK's one pool writer (``Fleet.apply``), the same code the
Rust core, the other language bindings and the ``cua`` CLI use.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from datetime import timedelta
from typing import Any, Optional, Sequence, Union

from cua_sandbox._sdk import InvalidArgument, native

Seconds = Union[int, float, timedelta, None]


class PoolSpecMismatch(InvalidArgument):
    """A named cloud pool's template differs from the sandbox fields given
    with it. The message holds a readable diff; pass
    ``CloudOptions(pool=..., apply=True)`` to update the pool's template
    instead, or omit the fields to use the pool as it is."""


class ClaimSecretsNotDelivered(TimeoutError):
    """A claim bound, but its per-claim secrets (the env token) never reached
    the sandbox within the bounded wait (90 s); the claim was released."""


def translate_native_error(error: BaseException) -> BaseException:
    """The cua-sandbox exception for a native ``CuaError`` (the error itself
    when it has no dedicated type)."""
    try:
        n = native()
    except ImportError:  # pragma: no cover - packaging error
        return error
    if isinstance(error, n.CuaError.PoolSpecMismatch):
        return PoolSpecMismatch(str(error))
    if isinstance(error, n.CuaError.ClaimSecretsNotDelivered):
        return ClaimSecretsNotDelivered(str(error))
    ambiguous = getattr(n.CuaError, "AmbiguousSandbox", None)
    if ambiguous is not None and isinstance(error, ambiguous):
        from cua_sandbox._refs import AmbiguousSandbox

        candidates = list(n.ambiguous_sandbox_candidates(str(error)))
        name = candidates[0].split(":", 1)[-1] if candidates else ""
        return AmbiguousSandbox(name, candidates)
    return error


def _seconds(value: Seconds, name: str) -> Optional[int]:
    if value is None:
        return None
    if isinstance(value, timedelta):
        value = value.total_seconds()
    if isinstance(value, bool) or not isinstance(value, (int, float)) or value < 0:
        raise ValueError(f"{name} must be a non-negative number of seconds or a timedelta")
    return int(value)


@dataclass
class SandboxSpec:
    """What a sandbox runs. Unset fields keep Fleet's defaults (and are not
    compared against a named pool's template).

    * ``image``: an :class:`~cua_sandbox.Image` (``Image.from_registry(ref,
      secret=...)`` carries its registry secret) or a registry reference.
    * ``command`` / ``args``: argv replacing the image ENTRYPOINT / CMD.
    * ``env``: plain environment variables (not secrets).
    * ``services``: named guest ports, ``{"mcp": 8765}``.
    * ``wait_for``: ``tcp("mcp")`` / ``http("mcp", "/health")``: a replica
      binds a claim only once it passes.
    * ``cpu`` / ``memory_mb`` (or ``memory="4GB"``).
    * ``sidecars``: :class:`~cua_sandbox.Container` (or image strings).
    * ``registry_secret``: a :class:`~cua_sandbox.RegistrySecret`, stored as
      the pool's ``cua-registry-*`` pull Secret; ``registry_secret_name``
      references an existing one.
    * ``process_mode``: ``"Legacy"`` or ``"Run"``.
    * ``claim_secrets``: claims may carry a per-claim env token.
    """

    image: Any = None
    command: Optional[Sequence[str]] = None
    args: Optional[Sequence[str]] = None
    env: dict[str, str] = field(default_factory=dict)
    services: dict[str, int] = field(default_factory=dict)
    wait_for: Any = None
    cpu: Optional[int] = None
    memory_mb: Optional[int] = None
    memory: Union[str, int, None] = None
    efi: bool = False
    sidecars: Sequence[Any] = ()
    registry_secret: Any = None
    registry_secret_name: Optional[str] = None
    process_mode: Optional[str] = None
    claim_secrets: bool = False

    def reference(self) -> str:
        """The image's registry reference (``""`` when unset)."""
        if self.image is None:
            return ""
        if isinstance(self.image, str):
            return self.image
        from cua_sandbox.image import cloud_registry_image

        reference = cloud_registry_image(self.image)
        if not reference:
            raise NotImplementedError(
                "Fleet cloud sandboxes require a supported built-in image or "
                "Image.from_registry(...)"
            )
        return reference

    def native(self) -> Any:
        """The ``cua.SandboxSpec`` record."""
        from cua_sandbox.containers import sidecars_of
        from cua_sandbox.options import probes

        n = native()
        memory_mb = self.memory_mb
        if self.memory is not None:
            if memory_mb is not None:
                raise ValueError("pass memory= or memory_mb=, not both")
            from cua_sandbox.options import parse_memory

            memory_mb = parse_memory(self.memory)
        readiness = probes(self.wait_for)
        if len(readiness) > 1:
            raise ValueError("a pool template takes one readiness probe (wait_for=)")
        secret = self.registry_secret
        if secret is None and self.image is not None and not isinstance(self.image, str):
            secret = getattr(self.image, "_secret", None)
        return n.SandboxSpec(
            image=self.reference(),
            command=list(self.command) if self.command else None,
            args=list(self.args) if self.args else None,
            env=dict(self.env or {}),
            services={k: int(v) for k, v in (self.services or {}).items()},
            readiness=readiness[0].native() if readiness else None,
            cpu=self.cpu,
            memory_mb=memory_mb,
            efi=self.efi
            or bool(
                self.image is not None
                and not isinstance(self.image, str)
                and self.image.os_type == "windows"
            ),
            sidecars=[c.native() for c in sidecars_of(self.sidecars)],
            registry_secret=secret.native() if secret is not None else None,
            registry_secret_name=self.registry_secret_name,
            process_mode=self.process_mode,
            claim_secrets=self.claim_secrets,
        )


@dataclass
class PoolOptions:
    """How a pool keeps capacity for a :class:`SandboxSpec`. Unset fields keep
    Fleet's defaults.

    * ``runtime``: ``"gvisor"`` or ``"kubevirt"`` (unset: from the image).
    * ``replicas``: size of a pool without autoscaling (default 1).
    * ``warm``: keep one sandbox ready (``minPoolSize: 1``).
    * ``min_pool_size`` / ``max_pool_size``: autoscaling floor / ceiling.
    * ``idle_ttl``: delete the pool after this long without claims.
    * ``ttl_policy``: ``"Retain"`` or ``"Cascade"`` (what TTL expiry deletes).
    * ``pool_ttl``: pool creation-age TTL.
    * ``claim_ttl``: default TTL of claims made on the pool.

    Durations are seconds or a ``timedelta``.
    """

    runtime: Optional[str] = None
    replicas: Optional[int] = None
    warm: Optional[bool] = None
    min_pool_size: Optional[int] = None
    max_pool_size: Optional[int] = None
    idle_ttl: Seconds = None
    ttl_policy: Optional[str] = None
    pool_ttl: Seconds = None
    claim_ttl: Seconds = None

    def native(self) -> Any:
        """The ``cua.PoolOptions`` record."""
        if self.runtime == "macos":
            raise ValueError(
                "macOS images run locally with Lume; Fleet does not offer macOS sandboxes in "
                "this SDK"
            )
        if self.runtime is not None and self.runtime not in ("gvisor", "kubevirt"):
            raise ValueError("runtime must be 'gvisor' or 'kubevirt'")
        return native().PoolOptions(
            runtime=self.runtime,
            replicas=self.replicas,
            warm=self.warm,
            min_pool_size=self.min_pool_size,
            max_pool_size=self.max_pool_size,
            idle_ttl_seconds=_seconds(self.idle_ttl, "idle_ttl"),
            ttl_policy=self.ttl_policy,
            pool_ttl_seconds=_seconds(self.pool_ttl, "pool_ttl"),
            claim_ttl_seconds=_seconds(self.claim_ttl, "claim_ttl"),
        )


@dataclass(frozen=True)
class PoolExport:
    """A pool read back as the shared model (``Pool.export``)."""

    name: str
    runtime: str
    spec: Any
    options: Any
    terraform: str


def native_fleet() -> Any:
    """The SDK's ``Fleet`` control plane (the process-wide embedded runtime)."""
    from cua_sandbox import _autopool

    return _autopool._native_cua().fleet()


async def call_native(coro: Any) -> Any:
    """Awaits a native call, raising the cua-sandbox exception types."""
    try:
        return await coro
    except Exception as error:  # noqa: BLE001 - re-raised, typed when known
        typed = translate_native_error(error)
        if typed is error:
            raise
        raise typed from error


def generate_claim_token() -> str:
    """A fresh per-claim env token (64 hex characters) for
    ``pool.claim(claim_token=...)``."""
    return str(native().fleet_generate_claim_token())

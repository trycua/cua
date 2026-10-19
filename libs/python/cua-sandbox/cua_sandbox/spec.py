"""Pool and sandbox spec types, retired with Cua Cloud.

Cua Cloud has ended, so there are no pools: every ``Pool`` call raises
:class:`~cua_sandbox._ended.CloudEnded`. The data classes and errors stay
importable so existing code keeps importing; the module goes away in the next
minor release.
"""

from __future__ import annotations

import secrets
from dataclasses import dataclass, field
from datetime import timedelta
from typing import Any, Optional, Sequence, Union

from cua_sandbox._sdk import InvalidArgument

Seconds = Union[int, float, timedelta, None]


class PoolSpecMismatch(InvalidArgument):
    """A named cloud pool's template differs from the sandbox fields given
    with it. Pools belonged to Cua Cloud, which has ended; nothing raises this
    any more."""


class ClaimSecretsNotDelivered(TimeoutError):
    """A claim bound, but its per-claim secrets (the env token) never reached
    the sandbox. Claims belonged to Cua Cloud, which has ended; nothing raises
    this any more."""


class PoolAccessDeniedError(PermissionError):
    """A pool or template operation was refused for this credential. Pools
    belonged to Cua Cloud, which has ended; nothing raises this any more."""


@dataclass
class SandboxSpec:
    """What a sandbox in a pool ran. Pools belonged to Cua Cloud, which has
    ended; this data class is kept so existing imports keep working.

    * ``image``: an :class:`~cua_sandbox.Image` or a registry reference.
    * ``command`` / ``args``: argv replacing the image ENTRYPOINT / CMD.
    * ``env``: plain environment variables (not secrets).
    * ``services``: named guest ports, ``{"mcp": 8765}``.
    * ``wait_for``: ``tcp("mcp")`` / ``http("mcp", "/health")``.
    * ``cpu`` / ``memory_mb`` (or ``memory="4GB"``).
    * ``sidecars``: :class:`~cua_sandbox.Container` (or image strings).
    * ``registry_secret``: a :class:`~cua_sandbox.RegistrySecret`;
      ``registry_secret_name`` references an existing one.
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


@dataclass
class PoolOptions:
    """How a pool kept capacity for a :class:`SandboxSpec`. Pools belonged to
    Cua Cloud, which has ended; this data class is kept so existing imports
    keep working.

    * ``runtime``: ``"gvisor"`` or ``"kubevirt"``.
    * ``replicas``: size of a pool without autoscaling.
    * ``warm``: keep one sandbox ready.
    * ``min_pool_size`` / ``max_pool_size``: autoscaling floor / ceiling.
    * ``idle_ttl``: delete the pool after this long without claims.
    * ``ttl_policy``: ``"Retain"`` or ``"Cascade"``.
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


@dataclass(frozen=True)
class PoolExport:
    """A pool read back as the shared model (``Pool.export``)."""

    name: str
    runtime: str
    spec: Any
    options: Any
    terraform: str


def generate_claim_token() -> str:
    """A fresh per-claim env token (64 hex characters)."""
    return secrets.token_hex(32)

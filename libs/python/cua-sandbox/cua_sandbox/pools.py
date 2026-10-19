"""Managed Fleet pools: Cua Cloud has ended (CUA-1854).

Every call raises :class:`~cua_sandbox._ended.CloudEnded`; the module goes
away in the next minor release.
"""

from dataclasses import dataclass, field
from datetime import datetime
from typing import Any, Optional

from cua_sandbox._ended import CloudEnded

MANAGED_PREFIX = "cua-auto-"
LEGACY_EPHEMERAL_PREFIX = "cua-eph-"


def is_managed_pool_name(name: str, prefix: str = MANAGED_PREFIX) -> bool:
    return name.startswith((prefix, MANAGED_PREFIX, LEGACY_EPHEMERAL_PREFIX))


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


async def list_pools(*args: Any, **kwargs: Any) -> list[ManagedPoolInfo]:
    raise CloudEnded()


async def list_claims(*args: Any, **kwargs: Any) -> list[ClaimInfo]:
    raise CloudEnded()


async def gc(*args: Any, **kwargs: Any) -> GcReport:
    raise CloudEnded()


async def gc_pools(*args: Any, **kwargs: Any) -> GcReport:
    raise CloudEnded()


__all__ = [
    "ClaimInfo",
    "GcReport",
    "ManagedPoolInfo",
    "gc",
    "gc_pools",
    "is_managed_pool_name",
    "list_claims",
    "list_pools",
]

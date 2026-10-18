"""Managed Fleet pools: Cua Cloud has ended (CUA-1854).

Every call raises :class:`~cua_sandbox._ended.CloudEnded`; the module goes
away in the next minor release.
"""

from typing import Any

from cua_sandbox._autopool import (
    ClaimInfo,
    GcReport,
    ManagedPoolInfo,
    is_managed_pool_name,
)
from cua_sandbox._ended import CloudEnded


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

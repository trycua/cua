"""Managed Fleet pools: list them and garbage-collect them.

``Sandbox.ephemeral(image, local=False)`` claims sandboxes from this
account's managed pools (``cua-auto-*``), one per image spec, created on
first use and scaled to zero when idle. Idle pools are deleted automatically
after ``CUA_FLEET_POOL_IDLE_GC``; these calls inspect them or collect now.

    from cua_sandbox import pools

    for pool in await pools.list_pools():
        print(pool.name, pool.claims, pool.last_used)
    report = await pools.gc(idle_after=1800)
"""

from cua_sandbox._autopool import (
    ClaimInfo,
    GcReport,
    ManagedPoolInfo,
    gc,
    gc_pools,
    is_managed_pool_name,
    list_claims,
    list_pools,
)

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

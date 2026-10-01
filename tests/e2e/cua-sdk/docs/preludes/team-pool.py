# Hidden docs prelude `team-pool`: the dedicated pool a page claims from
# (created on the fleets/guides/capacity-and-claims page), named by CUA_POOL_NAME;
# team-pool.subst.json puts that name where the page shows `my-team-desktop`.
import os as _os

from cua_sandbox import Image as _Image
from cua_sandbox import Pool as _Pool
from cua_sandbox import PoolOptions as _PoolOptions
from cua_sandbox import SandboxSpec as _Spec

await _Pool.apply(  # noqa: F704 - docs blocks allow top-level await
    _os.environ["CUA_POOL_NAME"],
    _Spec(image=_Image.linux(), services={"env": 3211}),
    _PoolOptions(replicas=1),
)

# Hidden docs prelude `pool-applied`: the `pool` an earlier section applied.
import os as _os

from cua_sandbox import Pool as _Pool
from cua_sandbox import PoolOptions as _PoolOptions
from cua_sandbox import SandboxSpec as _Spec

pool = await _Pool.apply(  # noqa: F704
    _os.environ.setdefault("CUA_POOL_NAME", "cua-e2e-docs-pool"),
    _Spec(image="python:3.12-slim"),
    _PoolOptions(replicas=1),
)

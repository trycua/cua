# Hidden docs prelude `pool-name`: the CUA_POOL_NAME a pool script asks the
# reader to export (the nightly fleet lane sets its own cua-e2e-* name).
import os as _os

_os.environ.setdefault("CUA_POOL_NAME", "cua-e2e-docs-pool")

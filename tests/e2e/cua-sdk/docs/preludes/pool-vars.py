# Hidden docs prelude `pool-vars`: the names a pool guide asks the reader to
# pick (IMAGE, POOL_NAME, CLAIM_NAME), and the imports its first section had.
import os as _os

from cua_sandbox import Image, Pool  # noqa: F401

IMAGE = "python:3.12-slim"
POOL_NAME = _os.environ.setdefault("CUA_POOL_NAME", "cua-e2e-docs-pool")
CLAIM_NAME = _os.environ.setdefault("CUA_CLAIM_NAME", "cua-e2e-docs-claim")

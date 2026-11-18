"""SandboxApi: the class-level calls shared by the synchronous `Sandbox`."""

from __future__ import annotations

from typing import Optional

from e2b._typing import Unpack
from e2b.connection_config import ApiParams
from e2b.sandbox.main import SandboxBase
from e2b.sandbox.sandbox_api import SandboxQuery
from e2b.sandbox_sync.paginator import SandboxPaginator


class SandboxApi(SandboxBase):
    @staticmethod
    def list(
        query: Optional[SandboxQuery] = None,
        limit: Optional[int] = None,
        next_token: Optional[str] = None,
        order=None,
        **opts: Unpack[ApiParams],
    ) -> SandboxPaginator:
        """The sandboxes this package created on your machines (a page at a
        time), by start time when `order` is `"asc"` or `"desc"`."""
        return SandboxPaginator(query, limit, next_token, order, **opts)

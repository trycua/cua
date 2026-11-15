"""SandboxApi: the class-level calls shared by `AsyncSandbox` (e2b's base of the
same name). Its `list` pages through the sandboxes created on your machines."""

from __future__ import annotations

from typing import Optional

from e2b._typing import Unpack
from e2b.connection_config import ApiParams
from e2b.sandbox.main import SandboxBase
from e2b.sandbox.sandbox_api import SandboxQuery
from e2b.sandbox_async.paginator import AsyncSandboxPaginator


class SandboxApi(SandboxBase):
    @staticmethod
    def list(
        query: Optional[SandboxQuery] = None,
        limit: Optional[int] = None,
        next_token: Optional[str] = None,
        order=None,
        **opts: Unpack[ApiParams],
    ) -> AsyncSandboxPaginator:
        """The sandboxes this package created on your machines (a page at a
        time: `limit`, `next_token`), by start time when `order` is `"asc"` or
        `"desc"`."""
        return AsyncSandboxPaginator(query, limit, next_token, order, **opts)

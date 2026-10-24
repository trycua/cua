"""SandboxPaginator and SnapshotPaginator: the synchronous list paginators."""

from __future__ import annotations

from typing import List, Optional

from e2b._typing import Unpack
from e2b.connection_config import ApiParams
from e2b.sandbox.sandbox_api import (
    SandboxInfo,
    SandboxPaginatorBase,
    SnapshotInfo,
    SnapshotPaginatorBase,
)
from e2b.sandbox_async.paginator import AsyncSandboxPaginator, AsyncSnapshotPaginator
from e2b.sandbox_sync._loop import Loop


class SandboxPaginator(SandboxPaginatorBase):
    """Paginator for listing sandboxes.

    Example:
    ```python
    paginator = Sandbox.list()

    while paginator.has_next:
        sandboxes = paginator.next_items()
        print(sandboxes)
    ```
    """

    def __init__(self, query=None, limit=None, next_token=None, order=None, **opts):
        super().__init__(query=query, limit=limit, next_token=next_token, order=order, **opts)
        self._inner = AsyncSandboxPaginator(query, limit, next_token, order, **opts)

    @property
    def has_next(self) -> bool:
        return self._inner.has_next

    @property
    def next_token(self) -> Optional[str]:
        return self._inner.next_token

    def next_items(self, **opts: Unpack[ApiParams]) -> List[SandboxInfo]:
        """The next page of sandboxes. Call only if `has_next` is `True`."""
        return Loop.run(self._inner.next_items(**opts))


class SnapshotPaginator(SnapshotPaginatorBase):
    """Paginator for listing snapshots."""

    def __init__(self, sandbox_id=None, limit=None, next_token=None, name=None, **opts):
        super().__init__(
            sandbox_id=sandbox_id, limit=limit, next_token=next_token, name=name, **opts
        )
        self._inner = AsyncSnapshotPaginator(sandbox_id, limit, next_token, name, **opts)

    @property
    def has_next(self) -> bool:
        return self._inner.has_next

    @property
    def next_token(self) -> Optional[str]:
        return self._inner.next_token

    def next_items(self, **opts: Unpack[ApiParams]) -> List[SnapshotInfo]:
        """The next page of snapshots. Call only if `has_next` is `True`."""
        return Loop.run(self._inner.next_items(**opts))

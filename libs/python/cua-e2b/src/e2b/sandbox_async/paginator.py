"""`AsyncSandbox.list()` and `AsyncSandbox.list_snapshots()` paginators.

e2b pages its list endpoints with a cursor. cua-e2b lists what the machines
report, orders it (start time, then id, so the order is stable), and serves
pages of `limit` items (100 by default) with an opaque cursor: `next_token` is
the position of the next page, `has_next` is false after the last one. A token
that this package did not issue is refused as e2b's API refuses one:
`SandboxException("400: Invalid next token")`, when the page is fetched.
"""

from __future__ import annotations

import base64
from datetime import datetime, timezone
from typing import List, Optional

from e2b import _wire
from e2b._backend import get_backend
from e2b._errors import map_error
from e2b._info import sandbox_info
from e2b._typing import Unpack
from e2b.connection_config import ApiParams, ConnectionConfig, merge_api_params
from e2b.exceptions import InvalidArgumentException
from e2b.sandbox.sandbox_api import (
    SandboxInfo,
    SandboxPaginatorBase,
    SandboxQuery,
    SnapshotInfo,
    SnapshotPaginatorBase,
)

DEFAULT_PAGE = 100
MAX_PAGE = 100


def _offset(next_token: Optional[str]) -> int:
    if not next_token:
        return 0
    try:
        raw = base64.urlsafe_b64decode(next_token.encode()).decode()
        if raw.startswith("cua:"):
            n = int(raw[4:])
            if n >= 0:
                return n
    except (ValueError, UnicodeDecodeError):
        pass
    raise _wire.bad_request("Invalid next token")


def _check_limit(limit) -> None:
    """The page size e2b's API accepts: 1 to 100 (`0` or `None`: its default)."""
    if limit is None or limit == 0:
        return
    if (
        not isinstance(limit, (int, float))
        or isinstance(limit, bool)
        or not float(limit).is_integer()
    ):
        raise _wire.invalid_param("limit", "value must be an integer")
    if limit < 1:
        raise _wire.invalid_param("limit", "number must be at least 1")
    if limit > MAX_PAGE:
        raise _wire.invalid_param("limit", f"number must be at most {MAX_PAGE}")


def _token(offset: int) -> str:
    return base64.urlsafe_b64encode(f"cua:{offset}".encode()).decode()


def _page(paginator, items: list) -> list:
    """The page `paginator` is at, and its cursor state moved on."""
    start = _offset(paginator._next_token)
    size = paginator.limit or DEFAULT_PAGE
    page = items[start : start + size]
    more = start + size < len(items)
    paginator._next_token = _token(start + size) if more else None
    paginator._has_next = more
    return page


class AsyncSandboxPaginator(SandboxPaginatorBase):
    """Paginator for listing sandboxes.

    Example:
    ```python
    paginator = AsyncSandbox.list()

    while paginator.has_next:
        sandboxes = await paginator.next_items()
        print(sandboxes)
    ```
    """

    def __init__(self, query=None, limit=None, next_token=None, order=None, **opts):
        super().__init__(query=query, limit=limit, next_token=next_token, order=order, **opts)

    async def next_items(self, **opts: Unpack[ApiParams]) -> List[SandboxInfo]:
        """The next page of sandboxes. Call only if `has_next` is `True`."""
        if not self.has_next:
            raise Exception("No more items to fetch")
        if self.order not in (None, "asc", "desc"):
            raise InvalidArgumentException(
                f"Invalid order {self.order!r}, expected 'asc' or 'desc'"
            )
        ConnectionConfig(**merge_api_params(self._opts, opts))
        _check_limit(self.limit)
        _offset(self._next_token)
        try:
            machines = await get_backend().list()
        except Exception as e:  # noqa: BLE001
            raise map_error(e)
        q = self.query or SandboxQuery()
        states = [str(s) for s in q.state] if q.state else None
        after = q.started_after.astimezone() if q.started_after else None
        out = []
        for m in machines:
            meta = dict(m.extra.get("metadata") or {}) if m.extra else {}
            started = m.started_at or datetime.now(timezone.utc)
            if states and m.state not in states:
                continue
            if q.metadata and any(meta.get(k) != v for k, v in q.metadata.items()):
                continue
            if q.template and q.template not in (m.template, (m.template or "").split(":")[0]):
                continue
            if after and started < after:
                continue
            out.append(sandbox_info(m))
        out.sort(key=lambda i: (i.started_at, i.sandbox_id), reverse=self.order != "asc")
        return _page(self, out)


class AsyncSnapshotPaginator(SnapshotPaginatorBase):
    """Paginator for listing snapshots.

    Example:
    ```python
    paginator = AsyncSandbox.list_snapshots()

    while paginator.has_next:
        snapshots = await paginator.next_items()
        print(snapshots)
    ```
    """

    def __init__(self, sandbox_id=None, limit=None, next_token=None, name=None, **opts):
        super().__init__(
            sandbox_id=sandbox_id, limit=limit, next_token=next_token, name=name, **opts
        )

    async def next_items(self, **opts: Unpack[ApiParams]) -> List[SnapshotInfo]:
        """The next page of snapshots. Call only if `has_next` is `True`."""
        if not self.has_next:
            raise Exception("No more items to fetch")
        ConnectionConfig(**merge_api_params(self._opts, opts))
        _offset(self._next_token)
        try:
            found = await get_backend().list_snapshots()
        except Exception as e:  # noqa: BLE001
            raise map_error(e)
        found.sort(key=lambda r: r.get("created") or "", reverse=True)
        out = [
            SnapshotInfo(snapshot_id=r["name"], names=[r["name"]])
            for r in found
            if (not self.sandbox_id or r.get("source") == self.sandbox_id)
            and (not self.name or self.name in (r["name"], r["name"].split(":")[0]))
        ]
        return _page(self, out)

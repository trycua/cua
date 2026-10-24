"""Cursor pagination state shared by the list calls."""

from __future__ import annotations

from typing import Generic, Mapping, Optional, TypeVar

from e2b._typing import Unpack
from e2b.connection_config import ApiParams

T = TypeVar("T")
OptsT = TypeVar("OptsT", bound=ApiParams)


class PaginatorBase(Generic[T, OptsT]):
    """Owns `has_next` / `next_token`; a concrete paginator implements
    `next_items`. `_update_pagination` reads the `x-next-token` header of a REST
    response, as E2B's list endpoints answer."""

    def __init__(
        self,
        limit: Optional[int] = None,
        next_token: Optional[str] = None,
        **opts: Unpack[OptsT],
    ):
        self._opts: OptsT = opts  # type: ignore[assignment]
        self.limit = limit
        self._has_next = True
        self._next_token = next_token

    @property
    def has_next(self) -> bool:
        """True if there are more items to fetch."""
        return self._has_next

    @property
    def next_token(self) -> Optional[str]:
        """The token to resume pagination with."""
        return self._next_token

    def _update_pagination(self, headers: Mapping[str, str]) -> None:
        self._next_token = headers.get("x-next-token")
        self._has_next = bool(self._next_token)

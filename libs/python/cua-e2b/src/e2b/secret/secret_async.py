"""`AsyncSecret`: E2B's secret store has no Cua counterpart; see `e2b.secret.base`."""

from __future__ import annotations

from typing import Dict, List, Optional

from e2b._typing import Unpack
from e2b.connection_config import ApiParams, merge_api_params
from e2b.secret.base import (
    SecretBase,
    SecretPaginatorBase,
    _check_connection,
    _no_secret_store,
    _validate_metadata,
    _validate_secret_name,
    _validate_secret_ref,
    _validate_secret_value,
)
from e2b.secret.types import SecretInfo


class AsyncSecretPaginator(SecretPaginatorBase):
    """Paginator for listing secrets.

    Example:
    ```python
    paginator = AsyncSecret.list()

    while paginator.has_next:
        secrets = await paginator.next_items()
    ```
    """

    async def next_items(self, **opts: Unpack[ApiParams]) -> List[SecretInfo]:
        """The next page of secrets. Raises `SecretException`: there is no secret store."""
        if not self.has_next:
            raise Exception("No more items to fetch")

        _check_connection(merge_api_params(self._opts, opts))
        raise _no_secret_store("list")


class AsyncSecret(SecretBase):
    """Module for managing secrets and workload identity helpers.

    Secret values are write-only. The store operations raise `SecretException`
    (there is no secret store behind cua-e2b); `fill` and `iam_token` work.
    """

    @classmethod
    async def create(
        cls,
        name: str,
        value: str,
        metadata: Optional[Dict[str, str]] = None,
        **opts: Unpack[ApiParams],
    ) -> SecretInfo:
        """Create a new secret and its first value (not possible, see the class)."""
        _check_connection(cls._resolve_api_params(**opts))
        _validate_secret_name(name)
        _validate_secret_value(value)
        _validate_metadata(metadata)
        raise _no_secret_store("create")

    @classmethod
    async def update(
        cls,
        secret: str,
        value: str,
        metadata: Optional[Dict[str, str]] = None,
        **opts: Unpack[ApiParams],
    ) -> SecretInfo:
        """Store a new version of a secret's value (not possible, see the class)."""
        _check_connection(cls._resolve_api_params(**opts))
        _validate_secret_ref(secret)
        _validate_secret_value(value)
        _validate_metadata(metadata)
        raise _no_secret_store("update")

    @classmethod
    async def get_info(cls, secret: str, **opts: Unpack[ApiParams]) -> SecretInfo:
        """A secret's metadata (not possible, see the class)."""
        _check_connection(cls._resolve_api_params(**opts))
        _validate_secret_ref(secret)
        raise _no_secret_store("get_info")

    @classmethod
    def list(
        cls,
        limit: Optional[int] = None,
        next_token: Optional[str] = None,
        **opts: Unpack[ApiParams],
    ) -> AsyncSecretPaginator:
        """A paginator over the project's secrets; its `next_items` raises
        `SecretException` (there is no secret store)."""
        return AsyncSecretPaginator(
            limit=limit,
            next_token=next_token,
            **cls._resolve_api_params(**opts),
        )

    @classmethod
    async def exists(cls, secret: str, **opts: Unpack[ApiParams]) -> bool:
        """Whether a secret exists (not answerable, see the class)."""
        _check_connection(cls._resolve_api_params(**opts))
        _validate_secret_ref(secret)
        raise _no_secret_store("exists")

    @classmethod
    async def destroy(cls, secret: str, **opts: Unpack[ApiParams]) -> bool:
        """Destroy a secret (not possible, see the class)."""
        _check_connection(cls._resolve_api_params(**opts))
        _validate_secret_ref(secret)
        raise _no_secret_store("destroy")

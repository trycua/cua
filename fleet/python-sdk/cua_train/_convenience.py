"""
TrainClient — an AuthenticatedClient that also exposes every generated
endpoint as a method, so callers write::

    c = TrainClient.from_key(token_url=..., client_id=..., client_secret=...)
    c.acquire_lanes(pool="test-pool", body=AcquireLanesRequest(n=8))
    c.submit_label_batch(pool="test-pool", label="run-1", body=BatchSubmitRequest(...))
    c.get_label_results(pool="test-pool", label="run-1", completed_only=True)

instead of importing each generated module and calling ``.sync`` by hand.

The endpoint surface is built generically: :func:`_load_operations` loops over
every module under ``cua_train.api`` once at import and maps each operation's
``sync`` function by name. ``TrainClient.__getattr__`` then resolves an
unknown attribute to that function with ``client=self`` pre-bound — no
per-endpoint code and no routing logic, so a new endpoint is callable the moment
it is regenerated from the OpenAPI spec. ``c`` remains a drop-in
``AuthenticatedClient`` for the low-level ``cua_train.api.*`` functions.
"""

from __future__ import annotations

import functools
import importlib
import pkgutil
import threading
import time
from collections.abc import Callable

import httpx

from . import api
from .client import AuthenticatedClient


def _load_operations() -> dict[str, Callable]:
    """Map ``operation_name -> sync()`` for every generated endpoint module.

    Walks ``cua_train/api/<tag>/<operation>.py`` (operation names are globally
    unique, so a flat namespace is safe). Run once at import.
    """
    ops: dict[str, Callable] = {}
    for tag in pkgutil.iter_modules(api.__path__):
        if not tag.ispkg:
            continue
        pkg = importlib.import_module(f"{api.__name__}.{tag.name}")
        for op in pkgutil.iter_modules(pkg.__path__):
            module = importlib.import_module(f"{pkg.__name__}.{op.name}")
            fn = getattr(module, "sync", None)
            if callable(fn):
                ops[op.name] = fn
    return ops


_OPERATIONS = _load_operations()


class TrainClient(AuthenticatedClient):
    """AuthenticatedClient with a ``client_credentials`` helper, a bearer token
    that is refreshed transparently before it expires, and every generated
    endpoint exposed as a method (its ``sync``, client pre-bound)."""

    @classmethod
    def from_key(
        cls,
        *,
        token_url: str,
        client_id: str,
        client_secret: str,
        base_url: str = "https://run.cua.ai",
        **kwargs,
    ) -> TrainClient:
        """Exchange client credentials for a bearer token and return a ready client.

        Args:
            token_url: Keycloak token endpoint, e.g.
                ``https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token``
            client_id: Service-account client ID (starts with ``key-``).
            client_secret: Client secret returned by ``POST /api/keys``.
            base_url: Backend base URL (default: production, run.cua.ai).
            **kwargs: Forwarded to :class:`AuthenticatedClient`.
        """
        token, ttl = cls._exchange(token_url, client_id, client_secret)
        self = cls(base_url=base_url, token=token, **kwargs)
        # Stash credentials so the token can be refreshed transparently.
        self._token_url = token_url
        self._client_id = client_id
        self._client_secret = client_secret
        self._token_deadline = time.monotonic() + ttl
        self._refresh_lock = threading.Lock()
        return self

    @staticmethod
    def _exchange(token_url: str, client_id: str, client_secret: str) -> tuple[str, float]:
        """client_credentials exchange → (access_token, lifetime_seconds)."""
        resp = httpx.post(
            token_url,
            data={
                "grant_type": "client_credentials",
                "client_id": client_id,
                "client_secret": client_secret,
            },
        )
        resp.raise_for_status()
        body = resp.json()
        return body["access_token"], float(body.get("expires_in", 300))

    def _refresh_token(self) -> None:
        """Re-mint the bearer token shortly before it expires. Transparent to
        callers, thread-safe (double-checked under a lock), and a no-op for
        clients not created via :meth:`from_key`."""
        if getattr(self, "_token_url", None) is None:
            return
        if time.monotonic() < self._token_deadline - 30:
            return
        with self._refresh_lock:
            if time.monotonic() < self._token_deadline - 30:
                return
            token, ttl = self._exchange(self._token_url, self._client_id, self._client_secret)
            self.token = token
            self._token_deadline = time.monotonic() + ttl
            header = f"{self.prefix} {token}" if self.prefix else token
            self._headers[self.auth_header_name] = header
            if self._client is not None:
                self._client.headers[self.auth_header_name] = header
            if self._async_client is not None:
                self._async_client.headers[self.auth_header_name] = header

    def get_httpx_client(self) -> httpx.Client:
        self._refresh_token()
        return super().get_httpx_client()

    def get_async_httpx_client(self) -> httpx.AsyncClient:
        self._refresh_token()
        return super().get_async_httpx_client()

    def __getattr__(self, name: str) -> Callable:
        # Only reached for attributes not found normally (slots/methods take
        # precedence), so this never shadows real client state. Unknown names —
        # including dunders probed by copy/pickle — raise AttributeError.
        fn = _OPERATIONS.get(name)
        if fn is None:
            raise AttributeError(
                f"{type(self).__name__!r} object has no attribute {name!r}; "
                f"available operations: {', '.join(sorted(_OPERATIONS))}"
            )
        return functools.partial(fn, client=self)

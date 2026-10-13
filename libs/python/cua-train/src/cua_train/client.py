"""TrainClient — authenticated httpx client for the Cua Cloud API."""

from __future__ import annotations

import threading
import time

import httpx


class TrainClient:
    """Authenticated client for the Cua Cloud API.

    Obtain via :meth:`from_key` using a per-user key (``ukey-…``) from
    your Cua Cloud account. The client transparently refreshes its bearer
    token before it expires.

    Both the control plane (VM claim lifecycle via ``/api/k8s``) and the
    data plane (exec/upload/screenshot via ``/api/svc``) are reached through
    the same base URL. There is no default: the legacy Fleets API (``run.cua.ai``) is retired,
    so ``base_url`` must be passed explicitly.
    """

    _DEFAULT_TOKEN_URL = "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token"
    _RETIRED_MESSAGE = (
        "cua-train no longer has a default API endpoint: the legacy Cua Fleets API "
        "(run.cua.ai) has been retired. Pass base_url=... to an API you run."
    )

    def __init__(
        self,
        token: str,
        base_url: str = "",
        *,
        token_url: str = _DEFAULT_TOKEN_URL,
        client_id: str = "",
        client_secret: str = "",
        token_deadline: float = 0.0,
    ) -> None:
        if not base_url:
            raise ValueError(self._RETIRED_MESSAGE)
        self._token = token
        self._base_url = base_url.rstrip("/")
        self._token_url = token_url
        self._client_id = client_id
        self._client_secret = client_secret
        self._token_deadline = token_deadline
        self._refresh_lock = threading.Lock()
        self._async_client: httpx.AsyncClient | None = None

    @classmethod
    def from_key(
        cls,
        *,
        token_url: str = _DEFAULT_TOKEN_URL,
        client_id: str,
        client_secret: str,
        base_url: str = "",
    ) -> "TrainClient":
        """Exchange client credentials for a bearer token.

        Args:
            token_url: Keycloak token endpoint.
            client_id: Per-user key client ID (``ukey-…``).
            client_secret: Client secret.
            base_url: Cua Cloud API base URL (required; the old run.cua.ai default is retired).
        """
        if not base_url:
            raise ValueError(cls._RETIRED_MESSAGE)
        token, ttl = cls._exchange(token_url, client_id, client_secret)
        return cls(
            token=token,
            base_url=base_url,
            token_url=token_url,
            client_id=client_id,
            client_secret=client_secret,
            token_deadline=time.monotonic() + ttl,
        )

    @staticmethod
    def _exchange(token_url: str, client_id: str, client_secret: str) -> tuple[str, float]:
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

    def reset_token(self) -> None:
        """Force re-exchange on the next :meth:`get_async_httpx_client` call."""
        self._token_deadline = 0

    def _refresh_token(self) -> None:
        if not self._client_id:
            return
        if time.monotonic() < self._token_deadline - 30:
            return
        with self._refresh_lock:
            if time.monotonic() < self._token_deadline - 30:
                return
            token, ttl = self._exchange(self._token_url, self._client_id, self._client_secret)
            self._token = token
            self._token_deadline = time.monotonic() + ttl
            auth_header = f"Bearer {token}"
            if self._async_client is not None:
                self._async_client.headers["Authorization"] = auth_header

    def get_async_httpx_client(self) -> httpx.AsyncClient:
        """Return an async httpx client with the current bearer token.

        Re-mints the token transparently when it is about to expire.
        """
        self._refresh_token()
        if self._async_client is None:
            self._async_client = httpx.AsyncClient(
                base_url=self._base_url,
                headers={"Authorization": f"Bearer {self._token}"},
                follow_redirects=False,
            )
        return self._async_client

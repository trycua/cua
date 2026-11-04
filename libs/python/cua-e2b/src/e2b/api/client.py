"""The REST client E2B generates for its control plane (`e2b.api.client`).

cua-e2b has no E2B REST control plane behind it: sandboxes, templates and volumes
are Cua Spaces, snapshots and Cua Volume, reached through the Cua SDK. So this
module keeps only the construction side of E2B's client: `AuthenticatedClient`
holds the base URL, headers, cookies, timeout and credentials it was built with and
copies them with `with_headers`, `with_cookies` and `with_timeout`, exactly as the
generated client does. Anything that would put a request on the wire raises
`SandboxException` and says what to use instead.
"""

from __future__ import annotations

from typing import Any, Dict, Optional, Union

from e2b.exceptions import SandboxException

NO_REST_MESSAGE = (
    "cua-e2b has no E2B REST control plane behind it: sandboxes are Cua Spaces and are "
    "managed through the Sandbox / AsyncSandbox, Template and Volume classes (Cua SDK), "
    "not through E2B's HTTP API. Call those instead of the generated REST client."
)


class AuthenticatedClient:
    """A client holding the connection data for E2B's REST API (never connected)."""

    def __init__(
        self,
        base_url: str,
        token: str,
        *,
        prefix: str = "Bearer",
        auth_header_name: str = "Authorization",
        cookies: Optional[Dict[str, str]] = None,
        headers: Optional[Dict[str, str]] = None,
        timeout: Any = None,
        verify_ssl: Union[str, bool, Any] = True,
        follow_redirects: bool = False,
        httpx_args: Optional[Dict[str, Any]] = None,
        raise_on_unexpected_status: bool = False,
    ):
        self._base_url = base_url
        self._cookies = dict(cookies) if cookies is not None else {}
        self._headers = dict(headers) if headers is not None else {}
        self._timeout = timeout
        self._verify_ssl = verify_ssl
        self._follow_redirects = follow_redirects
        self._httpx_args = httpx_args if httpx_args is not None else {}
        self._client: Any = None
        self._async_client: Any = None
        self.raise_on_unexpected_status = raise_on_unexpected_status
        self.token = token
        self.prefix = prefix
        self.auth_header_name = auth_header_name

    def _copy(self, **changes: Any) -> "AuthenticatedClient":
        other = object.__new__(type(self))
        other.__dict__.update(self.__dict__)
        other.__dict__.update(changes)
        other._client = None
        other._async_client = None
        return other

    def with_headers(self, headers: Dict[str, str]) -> "AuthenticatedClient":
        """A copy of this client with `headers` added to its headers."""
        return self._copy(_headers={**self._headers, **headers})

    def with_cookies(self, cookies: Dict[str, str]) -> "AuthenticatedClient":
        """A copy of this client with `cookies` added to its cookies."""
        return self._copy(_cookies={**self._cookies, **cookies})

    def with_timeout(self, timeout: Any) -> "AuthenticatedClient":
        """A copy of this client with another timeout."""
        return self._copy(_timeout=timeout)

    def set_httpx_client(self, client: Any) -> "AuthenticatedClient":
        """Keep a caller-built HTTP client; `get_httpx_client` returns it."""
        self._client = client
        return self

    def get_httpx_client(self) -> Any:
        if self._client is None:
            raise SandboxException(NO_REST_MESSAGE)
        return self._client

    def set_async_httpx_client(self, async_client: Any) -> "AuthenticatedClient":
        """Keep a caller-built async HTTP client; `get_async_httpx_client` returns it."""
        self._async_client = async_client
        return self

    def get_async_httpx_client(self) -> Any:
        if self._async_client is None:
            raise SandboxException(NO_REST_MESSAGE)
        return self._async_client


class Unset:
    """Marker of a REST field that was not set (falsy), as the generated client's."""

    def __bool__(self) -> bool:
        return False

    def __repr__(self) -> str:
        return "UNSET"


UNSET: Unset = Unset()

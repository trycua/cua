"""`ConnectionConfig` and the per-call API params of e2b, with the same attributes,
environment variables, defaults and validation errors.

cua-e2b has no E2B control plane: sandboxes are Cua Spaces reached through the Cua
SDK. The connection options that only configure E2B's REST/HTTP transport
(`api_key`, `domain`, `api_url`, `sandbox_url`, `proxy`, `http_version`, `retries`,
`headers`) are validated exactly as e2b does and kept on the config (and returned
by `get_api_params`), so code that reads or forwards them keeps working, but they
change nothing about how a Space is reached. `request_timeout` is real: it bounds
every call. `debug` and `domain` shape the URLs `get_host`, `get_sandbox_url` and
`get_sandbox_direct_url` return, as they do for e2b.
"""

from __future__ import annotations

import logging
import os
from typing import (
    Any,
    Dict,
    Literal,
    Mapping,
    Optional,
    Tuple,
    TypedDict,
    Union,
    cast,
)

from e2b._typing import Unpack
from e2b._version import package_version
from e2b.exceptions import InvalidArgumentException
from e2b.retry import resolve_max_retries
from e2b.sandbox_domains import is_supported_sandbox_domain

ProxyTypes = Union[str, Any]
"""The forms the `proxy` option accepts: a URL string, or an `httpx.URL` /
`httpx.Proxy` when httpx is installed. Retained, not used (there is no REST
transport)."""

REQUEST_TIMEOUT: float = 60.0  # 60 seconds
STREAM_INPUT_TIMEOUT: float = 60.0  # 60 seconds
DEFAULT_RETRIES = 3

# Idle bound for every read on a streaming file transfer.
READ_TIMEOUT: float = 60.0  # 60 seconds

KEEPALIVE_PING_INTERVAL_SEC = 50  # 50 seconds
KEEPALIVE_PING_HEADER = "Keepalive-Ping-Interval"

HttpVersion = Literal["1.1", "2"]
DEFAULT_HTTP_VERSION: HttpVersion = "2"
HTTP_VERSIONS: Tuple[HttpVersion, ...] = ("1.1", "2")


def resolve_http_version(http_version: Optional[str] = None) -> HttpVersion:
    """The HTTP version to use: the explicit value, else `E2B_HTTP_VERSION`, else
    `DEFAULT_HTTP_VERSION`. Rejects anything but `"1.1"` or `"2"`."""
    source = "http_version"
    if http_version is None:
        source = "E2B_HTTP_VERSION"
        http_version = os.getenv("E2B_HTTP_VERSION") or DEFAULT_HTTP_VERSION
    if http_version not in HTTP_VERSIONS:
        raise InvalidArgumentException(f"{source} must be '1.1' or '2', got {http_version!r}")
    return cast(HttpVersion, http_version)


class ApiParams(TypedDict, total=False):
    """Parameters for a request. For a sandbox they apply to all requests made to
    the returned sandbox."""

    request_timeout: Optional[float]
    """Timeout for the request in **seconds**, defaults to 60 seconds."""

    retries: Optional[int]
    """Control-plane retry count (a non-negative integer, default 3). Validated;
    cua-e2b has no REST control plane to retry against."""

    headers: Optional[Dict[str, str]]
    """Additional headers. Deprecated, use api_headers."""

    api_headers: Optional[Dict[str, str]]
    """Additional headers for E2B API requests."""

    api_key: Optional[str]
    """E2B API key, defaults to the `E2B_API_KEY` environment variable. Retained;
    Spaces are authorised by the Cua SDK."""

    validate_api_key: Optional[bool]
    """Deprecated; no effect."""

    domain: Optional[str]
    """E2B domain, defaults to `E2B_DOMAIN` or `e2b.app`."""

    api_url: Optional[str]
    """URL of the API, defaults to `https://api.<domain>`. Retained."""

    debug: Optional[bool]
    """Debug mode, defaults to `E2B_DEBUG`."""

    proxy: Optional[ProxyTypes]
    """Proxy for requests. Retained."""

    sandbox_url: Optional[str]
    """URL to connect to the sandbox, defaults to `E2B_SANDBOX_URL`."""

    http_version: Optional[HttpVersion]
    """`"1.1"` or `"2"`, defaults to `E2B_HTTP_VERSION` or `"2"`. Validated;
    retained."""


class ApiParamsWithLogger(ApiParams, total=False):
    """`ApiParams` plus the construction-time `logger`, returned by
    `ConnectionConfig.get_api_params`."""

    logger: Optional[logging.Logger]


def merge_api_params(bound_params: Optional[ApiParams], params: Mapping[str, object]) -> ApiParams:
    """Merge the API params bound to a class (by an `e2b.E2B` client) with the
    per-call params. Per-call params win, then the bound params, then the
    environment. A per-call `None` falls back to the bound value instead of clearing
    it."""
    if not bound_params:
        return cast(ApiParams, params)

    merged = cast(Dict[str, object], dict(bound_params))
    merged.update({k: v for k, v in params.items() if v is not None})
    return cast(ApiParams, merged)


class ClientFactory:
    """Base class of the resource classes (`Sandbox`, `Volume`, `Template`,
    `Secret`) whose classmethods build a `ConnectionConfig` from per-call params.
    An `e2b.E2B` client exposes subclasses with its own params bound."""

    _bound_api_params: ApiParams = {}

    @classmethod
    def _resolve_api_params(cls, **opts: Unpack[ApiParams]) -> ApiParams:
        return merge_api_params(cls._bound_api_params, opts)


class ConnectionConfig:
    """Configuration for the connection to the API."""

    envd_port = 49983

    _integration: Optional[str] = None

    @classmethod
    def set_integration(cls, integration: Optional[str]) -> None:
        """Append `integration` (e.g. `"e2b-code-interpreter/0.1.0"`) to the
        `User-Agent` of every config constructed afterwards. `None` clears it."""
        ConnectionConfig._integration = integration

    @staticmethod
    def _domain():
        return os.getenv("E2B_DOMAIN") or "e2b.app"

    @staticmethod
    def _debug():
        return os.getenv("E2B_DEBUG", "false").lower() == "true"

    @staticmethod
    def _api_key():
        return os.getenv("E2B_API_KEY")

    @staticmethod
    def _api_url():
        return os.getenv("E2B_API_URL")

    @staticmethod
    def _sandbox_url():
        return os.getenv("E2B_SANDBOX_URL")

    @staticmethod
    def _build_user_agent() -> str:
        parts = [f"e2b-python-sdk/{package_version}"]

        if ConnectionConfig._integration:
            parts.append(ConnectionConfig._integration)

        return " ".join(parts)

    def _apply_user_agent(
        self,
        headers: Dict[str, str],
        user_agent_override: Optional[str],
    ) -> bool:
        """Set the `User-Agent` on `headers`: an explicit value wins, else the
        SDK-built one. Returns whether the SDK-built value was applied."""
        if user_agent_override is not None:
            headers["User-Agent"] = user_agent_override
            return False

        headers["User-Agent"] = self._build_user_agent()
        return True

    def __init__(
        self,
        domain: Optional[str] = None,
        debug: Optional[bool] = None,
        api_key: Optional[str] = None,
        validate_api_key: Optional[bool] = None,
        api_url: Optional[str] = None,
        sandbox_url: Optional[str] = None,
        request_timeout: Optional[float] = None,
        headers: Optional[Dict[str, str]] = None,
        api_headers: Optional[Dict[str, str]] = None,
        extra_sandbox_headers: Optional[Dict[str, str]] = None,
        proxy: Optional[ProxyTypes] = None,
        logger: Optional[logging.Logger] = None,
        *,
        retries: Optional[int] = None,
        http_version: Optional[HttpVersion] = None,
    ):
        self.logger = logger
        self.domain = domain or ConnectionConfig._domain()
        self.debug = debug if debug is not None else ConnectionConfig._debug()
        self.api_key = api_key or ConnectionConfig._api_key()
        self.validate_api_key = validate_api_key
        self.headers = {**(headers or {}), **(api_headers or {})}
        self._user_agent_is_sdk_built = self._apply_user_agent(
            self.headers,
            self.headers.get("User-Agent"),
        )
        self.__extra_sandbox_headers = extra_sandbox_headers or {}

        self.proxy = proxy
        self.retries = resolve_max_retries(retries if retries is not None else DEFAULT_RETRIES)

        self.request_timeout = ConnectionConfig._get_request_timeout(
            REQUEST_TIMEOUT,
            request_timeout,
        )

        self.api_url = (
            api_url
            or ConnectionConfig._api_url()
            or ("http://localhost:3000" if self.debug else f"https://api.{self.domain}")
        )

        self._sandbox_url: Optional[str] = sandbox_url or ConnectionConfig._sandbox_url()
        self.http_version: HttpVersion = resolve_http_version(http_version)

    @staticmethod
    def _get_request_timeout(
        default_timeout: Optional[float],
        request_timeout: Optional[float],
    ):
        if request_timeout == 0:
            return None
        elif request_timeout is not None:
            return request_timeout
        else:
            return default_timeout

    def get_request_timeout(self, request_timeout: Optional[float] = None):
        return self._get_request_timeout(self.request_timeout, request_timeout)

    def get_sandbox_url(
        self, sandbox_id: str, sandbox_domain: str, port: Optional[int] = None
    ) -> str:
        port = self.envd_port if port is None else port

        if self._sandbox_url:
            return self._sandbox_url

        if self.debug:
            return f"http://{self.get_host(sandbox_id, sandbox_domain, port)}"

        sandbox_domain = sandbox_domain or self.domain
        if is_supported_sandbox_domain(sandbox_domain):
            return f"https://sandbox.{sandbox_domain}"

        return f"https://{self.get_host(sandbox_id, sandbox_domain, port)}"

    def get_sandbox_direct_url(
        self, sandbox_id: str, sandbox_domain: str, port: Optional[int] = None
    ) -> str:
        port = self.envd_port if port is None else port

        if self._sandbox_url:
            return self._sandbox_url

        if self.debug:
            return f"http://{self.get_host(sandbox_id, sandbox_domain, port)}"

        return f"https://{self.get_host(sandbox_id, sandbox_domain, port)}"

    def get_host(self, sandbox_id: str, sandbox_domain: str, port: int) -> str:
        """The host address to reach `port` of the sandbox from outside it."""
        if self.debug:
            return f"localhost:{port}"

        return f"{port}-{sandbox_id}.{sandbox_domain}"

    def get_api_params(
        self,
        **opts: Unpack[ApiParams],
    ) -> dict:
        """The parameters for an API call: this config's values, overridden by
        `opts`, as a copy (`api_url` is not overridable by the caller's ``None``)."""
        headers = opts.get("headers")
        api_headers = opts.get("api_headers")
        request_timeout = opts.get("request_timeout")
        api_key = opts.get("api_key")
        validate_api_key = opts.get("validate_api_key")
        api_url = opts.get("api_url")
        domain = opts.get("domain")
        debug = opts.get("debug")
        proxy = opts.get("proxy")
        sandbox_url = opts.get("sandbox_url")
        http_version = opts.get("http_version")
        retries = opts.get("retries")

        req_headers = self.headers.copy()
        if headers is not None:
            req_headers.update(headers)
        if api_headers is not None:
            req_headers.update(api_headers)
        if self._user_agent_is_sdk_built:
            # api_headers wins over headers, as in the merge above.
            per_call_user_agent = {**(headers or {}), **(api_headers or {})}.get("User-Agent")
            self._apply_user_agent(req_headers, per_call_user_agent)

        return dict(
            ApiParamsWithLogger(
                api_key=api_key if api_key is not None else self.api_key,
                validate_api_key=(
                    validate_api_key if validate_api_key is not None else self.validate_api_key
                ),
                api_url=api_url if api_url is not None else self.api_url,
                domain=domain if domain is not None else self.domain,
                debug=debug if debug is not None else self.debug,
                request_timeout=self.get_request_timeout(request_timeout),
                headers=req_headers,
                proxy=proxy if proxy is not None else self.proxy,
                sandbox_url=(
                    sandbox_url
                    if sandbox_url is not None
                    else cast(Optional[str], self._sandbox_url)
                ),
                http_version=(http_version if http_version is not None else self.http_version),
                logger=self.logger,
                retries=retries if retries is not None else self.retries,
            )
        )

    @property
    def sandbox_headers(self):
        """Headers sent to a sandbox: the User-Agent plus the extra sandbox
        headers (kept apart from the API headers, which carry the account key)."""
        return {
            "User-Agent": self.headers["User-Agent"],
            **self.__extra_sandbox_headers,
        }


Username = str
"""User used for the operation in the sandbox."""

default_username: Username = "user"
"""Default user used for the operation in the sandbox."""

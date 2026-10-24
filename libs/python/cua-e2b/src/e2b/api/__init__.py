"""E2B's REST client layer, as far as it has meaning on Cua.

Implemented for real (pure helpers): `encode_path_param`, `api_exception_from_code`
and `handle_api_exception` (HTTP status to exception class and message),
`proxy_to_config` / `ProxyConfig`, `make_logging_event_hooks`,
`SandboxCreateResponse`, `validate_api_key` (2.42).

Native error: `ApiClient` / `AsyncApiClient` can be constructed (they check the API
key and keep the connection data as E2B's do) but there is no E2B REST control plane
behind cua-e2b, so asking them for an HTTP client raises `SandboxException`
naming the classes to use (see `e2b.api.client`).
"""

from __future__ import annotations

import json
import logging
import os
import re
from dataclasses import dataclass
from types import TracebackType
from typing import Any, NamedTuple, Optional, Protocol, Tuple, Union
from urllib.parse import quote

# Importing the submodule also binds `e2b.api.client`, which `e2b` re-exports.
from e2b.api.client import AuthenticatedClient
from e2b.api.metadata import default_headers
from e2b.connection_config import ConnectionConfig, ProxyTypes
from e2b.exceptions import (
    AuthenticationException,
    InvalidArgumentException,
    RateLimitException,
    SandboxException,
    ServiceBusyException,
)


def encode_path_param(value: str) -> str:
    """Percent-encode a template ID, name or alias for use as one URL path
    segment: `namespace/name:tag` stays inside a single segment."""
    return quote(value, safe="")


def make_logging_event_hooks(log: Optional[logging.Logger]) -> dict:
    """Synchronous `event_hooks` that log requests and responses to `log`:
    requests at INFO, responses below 400 at INFO and from 400 at ERROR. No hooks
    when `log` is None."""
    if log is None:
        return {}

    def on_request(request) -> None:
        log.info(f"Request {request.method} {request.url}")

    def on_response(response) -> None:
        if response.status_code >= 400:
            log.error(f"Response {response.status_code}")
        else:
            log.info(f"Response {response.status_code}")

    return {"request": [on_request], "response": [on_response]}


def make_async_logging_event_hooks(log: Optional[logging.Logger]) -> dict:
    """Asynchronous counterpart of `make_logging_event_hooks`."""
    if log is None:
        return {}

    async def on_request(request) -> None:
        log.info(f"Request {request.method} {request.url}")

    async def on_response(response) -> None:
        if response.status_code >= 400:
            log.error(f"Response {response.status_code}")
        else:
            log.info(f"Response {response.status_code}")

    return {"request": [on_request], "response": [on_response]}


connection_retries = int(os.getenv("E2B_CONNECTION_RETRIES") or "3")

# Pool tuning of E2B's transports, read from the same environment variables.
pool_idle_timeout = float(os.getenv("E2B_KEEPALIVE_EXPIRY") or "300")
pool_max_idle_per_host = int(os.getenv("E2B_MAX_KEEPALIVE_CONNECTIONS") or "20")
max_connections = max(1, int(os.getenv("E2B_MAX_CONNECTIONS") or "200"))


class ProxyConfig(NamedTuple):
    """The `proxy` connection option as a hashable tuple: a proxy URL (scheme http,
    https, socks5 or socks5h), basic-auth credentials and proxy headers."""

    url: str
    auth: Optional[Tuple[str, str]] = None
    headers: Tuple[Tuple[str, str], ...] = ()

    def to_pyqwest(self) -> Any:
        """The `pyqwest.Proxy` for this config; needs `pyqwest`, which only E2B's
        own transport uses."""
        try:
            from pyqwest import Proxy  # type: ignore[import-not-found]
        except ImportError as e:
            raise SandboxException(
                "ProxyConfig.to_pyqwest needs the 'pyqwest' package, which only E2B's REST "
                "transport uses; cua-e2b has no such transport (the proxy is kept as a "
                "ProxyConfig on the connection config)."
            ) from e
        return Proxy(self.url, auth=self.auth, headers=self.headers or None)


def _is(obj: Any, module_prefix: str, name: str) -> bool:
    cls = type(obj)
    return cls.__name__ == name and cls.__module__.split(".")[0] == module_prefix


def proxy_to_config(proxy: Optional[ProxyTypes]) -> Optional[ProxyConfig]:
    """Convert the `proxy` option -- a URL string, an `httpx.URL` or an
    `httpx.Proxy` -- to a `ProxyConfig`. An `httpx.Proxy` `ssl_context` has no
    counterpart and is rejected rather than silently dropped."""
    if proxy is None:
        return None
    if isinstance(proxy, str):
        return ProxyConfig(proxy)
    if _is(proxy, "httpx", "URL"):
        return ProxyConfig(str(proxy))
    if _is(proxy, "httpx", "Proxy"):
        if proxy.ssl_context is not None:
            raise InvalidArgumentException("httpx.Proxy ssl_context is not supported")
        return ProxyConfig(
            str(proxy.url),
            auth=proxy.auth,
            headers=tuple(proxy.headers.items()),
        )
    raise InvalidArgumentException(
        "Only URL-string, httpx.URL, and httpx.Proxy proxies are supported, "
        'e.g. proxy="http://user:pass@localhost:8030"'
    )


@dataclass
class SandboxCreateResponse:
    sandbox_id: str
    sandbox_domain: Optional[str]
    envd_version: str
    envd_access_token: Optional[str]
    traffic_access_token: Optional[str]


def api_exception_from_code(
    status_code: int,
    message: Optional[str] = None,
    default_exception_class: type[Exception] = SandboxException,
    stack_trace: Optional[TracebackType] = None,
) -> Exception:
    """The exception for an API error code and message: 401 is
    `AuthenticationException`, 429 `RateLimitException`, 503 `ServiceBusyException`;
    anything else `default_exception_class` carrying the status code."""
    if status_code == 401:
        text = f"{status_code}: Unauthorized, please check your credentials."
        if message:
            text += f" - {message}"
        return AuthenticationException(text)

    if status_code == 429:
        text = f"{status_code}: Rate limit exceeded, please try again later."
        if message:
            text += f" - {message}"
        return RateLimitException(text)

    if status_code == 503:
        text = f"{status_code}: Service temporarily unavailable, please retry."
        if message:
            text += f" - {message}"
        return ServiceBusyException(text)

    err = default_exception_class(f"{status_code}: {message}").with_traceback(stack_trace)
    if isinstance(err, SandboxException):
        err.status_code = status_code

    return err


def handle_api_exception(
    e: "SupportsApiErrorResponse",
    default_exception_class: type[Exception] = SandboxException,
    stack_trace: Optional[TracebackType] = None,
):
    """The exception for an error response (`status_code`, JSON `content` with an
    optional `message`)."""
    try:
        body = json.loads(e.content) if e.content else {}
    except json.JSONDecodeError:
        body = {}

    message = body["message"] if "message" in body else None
    if message is None and e.status_code not in (401, 429, 503):
        err = default_exception_class(f"{e.status_code}: {e.content}").with_traceback(stack_trace)
        if isinstance(err, SandboxException):
            err.status_code = e.status_code

        return err

    return api_exception_from_code(e.status_code, message, default_exception_class, stack_trace)


class SupportsApiErrorResponse(Protocol):
    @property
    def status_code(self) -> int: ...

    @property
    def content(self) -> Union[str, bytes]: ...


_API_KEY_PATTERN = re.compile(r"\Ae2b_[0-9a-f]+\Z")
_API_KEY_EXAMPLE = "e2b_" + "0" * 40


def validate_api_key(api_key: str) -> None:
    """e2b 2.42: an API key is `e2b_` followed by hex characters (later releases no
    longer check). Raises `AuthenticationException` otherwise."""
    if not _API_KEY_PATTERN.match(api_key):
        raise AuthenticationException(
            'Invalid API key format: expected "e2b_" followed by hex '
            f'characters (e.g. "{_API_KEY_EXAMPLE}"). '
            "Visit the API Keys tab at https://e2b.dev/dashboard?tab=keys to get your API key."
        )


class ApiClient(AuthenticatedClient):
    """The client for E2B's API, built from a `ConnectionConfig`. It holds the
    connection data; there is no REST control plane to send it to (see the module
    docstring), so `get_httpx_client()` raises `SandboxException`."""

    def __init__(
        self,
        config: ConnectionConfig,
        transport: Optional[Any] = None,
        *args,
        **kwargs,
    ):
        self._proxy = config.proxy

        if config.api_key is None:
            raise AuthenticationException(
                "API key is required, please visit the API Keys tab at https://e2b.dev/dashboard?tab=keys to get your API key. "
                "You can either set the environment variable `E2B_API_KEY` "
                'or you can pass it directly to the method like api_key="e2b_..."',
            )

        token = config.api_key
        self._logger = config.logger
        self.config = config

        headers = {**default_headers, **(config.headers or {})}
        more_headers: Optional[dict] = kwargs.pop("headers", None)
        if more_headers:
            headers.update(more_headers)
        kwargs.pop("token", None)
        kwargs.pop("auth_header_name", None)
        kwargs.pop("prefix", None)

        httpx_args: dict = {"event_hooks": self._logging_event_hooks()}
        if transport is not None:
            httpx_args["transport"] = transport
        else:
            httpx_args["proxy"] = config.proxy

        kwargs.setdefault("timeout", config.request_timeout)

        super().__init__(
            *args,
            base_url=config.api_url,
            httpx_args=httpx_args,
            headers=headers,
            token=token or "",
            auth_header_name="X-API-KEY",
            prefix="",
            **kwargs,
        )

    def _logging_event_hooks(self) -> dict:
        return make_logging_event_hooks(self._logger)


class AsyncApiClient(ApiClient):
    def _logging_event_hooks(self) -> dict:
        return make_async_logging_event_hooks(self._logger)

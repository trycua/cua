"""Connection settings of a volume.

cua-e2b reaches a volume through the Cua SDK (your own Cua Volume), not over
HTTP to a volume content host, so these settings only travel with the
objects: they are resolved from the same options and environment variables as
e2b's (`E2B_DOMAIN`, `E2B_DEBUG`, `E2B_VOLUME_API_URL`, `E2B_HTTP_VERSION`)
and kept, and no request uses them.

The e2b SDK is MIT-licensed; this module matches its public API.
"""

import logging
import os
from typing import Dict, Optional, TypedDict

from e2b.connection_config import HttpVersion, ProxyTypes, resolve_http_version

REQUEST_TIMEOUT: float = 60.0  # 60 seconds

# Timeout for volume file transfers (1 hour).
FILE_TIMEOUT: float = 3600.0

# Idle bound for reads of a streamed transfer.
READ_TIMEOUT: float = 60.0


class VolumeApiParams(TypedDict, total=False):
    """Parameters of a call on a volume."""

    domain: Optional[str]
    """Domain of the volume API, defaults to `E2B_DOMAIN` or `e2b.app`."""

    debug: Optional[bool]
    """Debug mode, defaults to the `E2B_DEBUG` environment variable."""

    request_timeout: Optional[float]
    """Timeout of the request in **seconds**, defaults to 60 seconds."""

    headers: Optional[Dict[str, str]]
    """Additional headers."""

    token: Optional[str]
    """Volume auth token."""

    api_url: Optional[str]
    """URL of the volume API, defaults to `E2B_VOLUME_API_URL` or `https://api.<domain>`."""

    proxy: Optional[ProxyTypes]
    """Proxy for the request."""

    http_version: Optional[HttpVersion]
    """HTTP version, defaults to the `E2B_HTTP_VERSION` environment variable or `"2"`."""

    logger: Optional[logging.Logger]
    """Logger for request and response logging."""


class VolumeConnectionConfig:
    """Configuration for the volume content API."""

    @staticmethod
    def _domain():
        return os.getenv("E2B_DOMAIN") or "e2b.app"

    @staticmethod
    def _debug():
        return os.getenv("E2B_DEBUG", "false").lower() == "true"

    @staticmethod
    def _volume_api_url():
        return os.getenv("E2B_VOLUME_API_URL")

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

    def __init__(
        self,
        domain: Optional[str] = None,
        debug: Optional[bool] = None,
        token: Optional[str] = None,
        api_url: Optional[str] = None,
        request_timeout: Optional[float] = None,
        headers: Optional[Dict[str, str]] = None,
        proxy: Optional[ProxyTypes] = None,
        logger: Optional[logging.Logger] = None,
        *,
        http_version: Optional[HttpVersion] = None,
    ):
        self.logger = logger
        self.domain = domain or self._domain()
        self.debug = debug if debug is not None else self._debug()
        self.api_url = (
            api_url
            or self._volume_api_url()
            or ("http://localhost:8080" if self.debug else f"https://api.{self.domain}")
        )
        self.access_token = token
        self.token = self.access_token
        self.proxy = proxy
        self.http_version: HttpVersion = resolve_http_version(http_version)

        from e2b import __version__

        self.headers = dict(headers) if headers else {}
        self.headers["User-Agent"] = f"cua-e2b/{__version__}"
        self.request_timeout = self._get_request_timeout(REQUEST_TIMEOUT, request_timeout)

    def get_request_timeout(self, request_timeout: Optional[float] = None):
        return self._get_request_timeout(self.request_timeout, request_timeout)

    def get_api_params(self, **opts) -> dict:
        """The settings with the per-call `opts` laid over them."""
        req_headers = self.headers.copy()
        if opts.get("headers") is not None:
            req_headers.update(opts["headers"])

        def pick(name, default):
            return opts[name] if opts.get(name) is not None else default

        return dict(
            VolumeApiParams(
                domain=pick("domain", self.domain),
                debug=pick("debug", self.debug),
                token=pick("token", self.token),
                api_url=pick("api_url", self.api_url),
                request_timeout=self.get_request_timeout(opts.get("request_timeout")),
                headers=req_headers,
                proxy=pick("proxy", self.proxy),
                http_version=pick("http_version", self.http_version),
                logger=pick("logger", self.logger),
            )
        )

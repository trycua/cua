"""OrcaRouter authentication and API origins.

Authentication and inference are different public origins:

* ``https://www.orcarouter.ai`` serves the consent screen (``/auth``) and the
  code exchange (``/api/v1/auth/keys``);
* ``https://api.orcarouter.ai/v1`` serves inference and model discovery.

One public origin is never derived from the other by rewriting a hostname or by
appending ``/v1``; a self-hosted deployment configures ``ORCA_BASE_URL`` as a
shared origin, and the explicit ``ORCA_AUTH_BASE_URL`` / ``ORCA_API_BASE_URL``
overrides take precedence over it.
"""

from __future__ import annotations

import os
from collections.abc import Mapping
from urllib.parse import urlsplit

DEFAULT_AUTH_BASE_URL = "https://www.orcarouter.ai"
DEFAULT_API_BASE_URL = "https://api.orcarouter.ai/v1"

#: Environment variable names, exported so callers never re-spell them.
ORCA_BASE_URL_ENV = "ORCA_BASE_URL"
ORCA_AUTH_BASE_URL_ENV = "ORCA_AUTH_BASE_URL"
ORCA_API_BASE_URL_ENV = "ORCA_API_BASE_URL"
ORCA_KEY_ENV = "ORCA_KEY"

AUTHORIZE_PATH = "/auth"
EXCHANGE_PATH = "/api/v1/auth/keys"

_LOOPBACK_HOSTS = frozenset({"127.0.0.1", "localhost", "::1", "[::1]"})


class OrcaRouterConfigError(ValueError):
    """Raised when a configured OrcaRouter origin is unusable."""


def is_loopback_host(host: str) -> bool:
    """Return whether ``host`` denotes the local machine."""
    return host.lower() in _LOOPBACK_HOSTS


def normalize_origin(value: str, *, setting: str) -> str:
    """Validate and normalize a base URL.

    Remote origins must use HTTPS. Plain HTTP is accepted only for loopback
    development origins. Userinfo and fragments are rejected.
    """
    candidate = value.strip()
    if not candidate:
        raise OrcaRouterConfigError(f"{setting} is empty.")
    parts = urlsplit(candidate)
    if parts.scheme not in ("http", "https"):
        raise OrcaRouterConfigError(
            f"{setting} must be an http(s) URL, got {parts.scheme or 'no scheme'!r}."
        )
    if parts.username or parts.password:
        raise OrcaRouterConfigError(f"{setting} must not contain userinfo.")
    if parts.fragment:
        raise OrcaRouterConfigError(f"{setting} must not contain a fragment.")
    if not parts.netloc:
        raise OrcaRouterConfigError(f"{setting} must include a host.")
    if parts.scheme == "http" and not is_loopback_host(parts.hostname or ""):
        raise OrcaRouterConfigError(
            f"{setting} must use https for non-loopback origins; http is allowed only "
            f"for 127.0.0.1, [::1] or localhost."
        )
    return candidate.rstrip("/")


def _read(environ: Mapping[str, str], name: str) -> str | None:
    value = environ.get(name)
    if value is None:
        return None
    stripped = value.strip()
    return stripped or None


def resolve_auth_base_url(environ: Mapping[str, str] | None = None) -> str:
    """Resolve the authentication origin.

    ``ORCA_AUTH_BASE_URL`` wins, then the shared ``ORCA_BASE_URL``, then the
    public default.
    """
    source = os.environ if environ is None else environ
    explicit = _read(source, ORCA_AUTH_BASE_URL_ENV)
    if explicit:
        return normalize_origin(explicit, setting=ORCA_AUTH_BASE_URL_ENV)
    shared = _read(source, ORCA_BASE_URL_ENV)
    if shared:
        return normalize_origin(shared, setting=ORCA_BASE_URL_ENV)
    return DEFAULT_AUTH_BASE_URL


def _with_v1(origin: str) -> str:
    """Append ``/v1`` to a shared self-hosted origin once."""
    if origin.endswith("/v1"):
        return origin
    return f"{origin}/v1"


def resolve_api_base_url(environ: Mapping[str, str] | None = None) -> str:
    """Resolve the inference/catalog origin, including its ``/v1`` suffix.

    ``ORCA_API_BASE_URL`` wins, then the shared ``ORCA_BASE_URL`` (with ``/v1``
    appended for a single-origin self-hosted deployment), then the public
    default.
    """
    source = os.environ if environ is None else environ
    explicit = _read(source, ORCA_API_BASE_URL_ENV)
    if explicit:
        return normalize_origin(explicit, setting=ORCA_API_BASE_URL_ENV)
    shared = _read(source, ORCA_BASE_URL_ENV)
    if shared:
        return _with_v1(normalize_origin(shared, setting=ORCA_BASE_URL_ENV))
    return DEFAULT_API_BASE_URL


def authorize_url(auth_base_url: str) -> str:
    """Return the consent-screen URL. Never derived from the API origin."""
    return f"{auth_base_url.rstrip('/')}{AUTHORIZE_PATH}"


def exchange_url(auth_base_url: str) -> str:
    """Return the code-exchange URL.

    The relay lives on ``api.orcarouter.ai`` at ``/v1``; the auth endpoints do
    not, so this URL is always built from the authentication origin.
    """
    return f"{auth_base_url.rstrip('/')}{EXCHANGE_PATH}"


def models_url(api_base_url: str, capability: str | None = None) -> str:
    """Return the model-catalog URL, optionally filtered by capability."""
    url = f"{api_base_url.rstrip('/')}/models"
    if capability:
        return f"{url}?capability={capability}"
    return url

"""Global configuration for cloud sandbox provisioning."""

from __future__ import annotations

import os
from dataclasses import dataclass
from typing import Optional

_DEFAULT_BASE_URL = "https://api.cua.ai"
_DEFAULT_FLEET_BASE_URL = "https://run.cua.ai"
_DEFAULT_TOKEN_URL = "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token"


@dataclass
class _Config:
    api_key: Optional[str] = None
    base_url: str = _DEFAULT_BASE_URL
    fleet_base_url: str = _DEFAULT_FLEET_BASE_URL
    token_url: str = _DEFAULT_TOKEN_URL
    client_id: Optional[str] = None
    client_secret: Optional[str] = None
    fleet_token: Optional[str] = None


_global_config = _Config()


def configure(
    *,
    api_key: Optional[str] = None,
    base_url: Optional[str] = None,
    fleet_base_url: Optional[str] = None,
    token_url: Optional[str] = None,
    client_id: Optional[str] = None,
    client_secret: Optional[str] = None,
    fleet_token: Optional[str] = None,
) -> None:
    """Set global configuration for cloud sandboxes.

    API-key cloud operations use ``base_url``. Fleet uses ``fleet_base_url`` and
    OAuth client credentials.
    """
    if api_key is not None:
        _global_config.api_key = api_key
    if base_url is not None:
        _global_config.base_url = base_url
    if fleet_base_url is not None:
        _global_config.fleet_base_url = fleet_base_url
    if token_url is not None:
        _global_config.token_url = token_url
    if client_id is not None:
        _global_config.client_id = client_id
    if client_secret is not None:
        _global_config.client_secret = client_secret
    if fleet_token is not None:
        _global_config.fleet_token = fleet_token


def get_api_key(override: Optional[str] = None) -> Optional[str]:
    """Resolve a legacy API key with per-call configuration taking priority."""
    if override:
        return override
    if _global_config.api_key:
        return _global_config.api_key
    credential = _read_credentials_key()
    if credential:
        return credential
    return os.environ.get("CUA_API_KEY")


def get_base_url() -> str:
    return os.environ.get("CUA_BASE_URL") or _global_config.base_url


def get_fleet_base_url() -> str:
    """Return the Fleet API endpoint without changing legacy VM API routing."""
    return os.environ.get("CUA_FLEET_BASE_URL") or _global_config.fleet_base_url


def get_fleet_token() -> Optional[str]:
    """Resolve a configured or environment-supplied Fleet workload token."""
    for token in (_global_config.fleet_token, os.environ.get("FLEETS_TOKEN")):
        if token:
            token = token.strip()
            if token:
                return token
    return None


def has_fleet_auth() -> bool:
    """Return whether static Fleet token or client credential auth is available."""
    return bool(get_fleet_token() or (get_client_id() and get_client_secret()))


#: The one "no Fleet credentials" message (``cua_fleet::MISSING_CREDENTIALS``).
FLEET_CREDENTIALS_MISSING = (
    "Fleet credentials missing: run `cua auth login` or set "
    "CUA_CLIENT_ID/CUA_CLIENT_SECRET, or pass local=True"
)


def has_fleet_session() -> bool:
    """Whether a ``cua auth login`` session is stored (no network access).

    Fleet uses it when no token or client credentials are configured. Set
    ``CUA_FLEET_SESSION=0`` to ignore it.
    """
    if os.environ.get("CUA_FLEET_SESSION", "").strip().lower() in ("0", "false", "off", "no"):
        return False
    try:
        from cua_sandbox._sdk import sdk

        return bool(sdk().embedded(fleet_from_env=False).auth().status().logged_in)
    except Exception:  # noqa: BLE001 - no SDK or unreadable store: no session
        return False


def may_have_fleet_session() -> bool:
    """Whether a ``cua auth login`` session may be stored, decided WITHOUT
    reading the OS credential vault: the SDK's non-secret session marker
    (``~/.cua/session.json``, written at login and on the first read of an
    older session) or the file store's file. Implicit calls (the default
    ``Sandbox.list()``) check this first; explicit cloud calls read the
    session (:func:`has_fleet_session`), which writes the marker."""
    try:
        from cua_sandbox._sdk import native

        return bool(native().may_have_fleet_session())
    except Exception:  # noqa: BLE001 - no SDK: no session to find
        return False


def fleet_auth_source(*, read_session: bool = True) -> Optional[str]:
    """Where Fleet credentials come from, in the order the SDK uses them.

    ``"FLEETS_TOKEN"`` (a workload token), ``"client credentials"``
    (``CUA_CLIENT_ID``/``CUA_CLIENT_SECRET``), ``"cua auth login session"``
    (the stored session, refreshed by the SDK while it is used) or ``None``
    when nothing is configured. A stored session is only looked up when
    neither of the first two is set. ``read_session=False`` decides that from
    the non-secret session marker alone, without reading the OS credential
    vault (for dashboards and other implicit checks).
    """
    if get_fleet_token():
        return "FLEETS_TOKEN"
    if get_client_id() and get_client_secret():
        return "client credentials"
    found = has_fleet_session() if read_session else may_have_fleet_session()
    return "cua auth login session" if found else None


def has_fleet_access() -> bool:
    """Fleet credentials, or a ``cua auth login`` session to fall back to."""
    return has_fleet_auth() or has_fleet_session()


def get_token_url() -> str:
    return os.environ.get("CUA_TOKEN_URL") or _global_config.token_url


def get_client_id(override: Optional[str] = None) -> Optional[str]:
    return override or _global_config.client_id or os.environ.get("CUA_CLIENT_ID")


def get_client_secret(override: Optional[str] = None) -> Optional[str]:
    return override or _global_config.client_secret or os.environ.get("CUA_CLIENT_SECRET")


def _read_credentials_key() -> Optional[str]:
    """Read a legacy API key from ``$CUA_HOME/credentials`` when present."""
    from cua_sandbox._paths import cua_home

    credential_path = cua_home() / "credentials"
    try:
        with open(credential_path) as credential_file:
            for line in credential_file:
                line = line.strip()
                if line.startswith("api_key="):
                    return line[len("api_key=") :]
                if line.startswith("api_key ="):
                    return line[len("api_key =") :].strip()
    except FileNotFoundError:
        pass
    return None

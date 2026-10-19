"""Process-wide configuration for the legacy API-key helpers."""

from __future__ import annotations

import os
import warnings
from dataclasses import dataclass
from typing import Optional

_DEFAULT_BASE_URL = "https://api.cua.ai"


@dataclass
class _Config:
    api_key: Optional[str] = None
    base_url: str = _DEFAULT_BASE_URL


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
    """Set global configuration.

    ``api_key`` and ``base_url`` apply to the legacy ``login()`` and
    ``whoami()`` helpers. ``fleet_base_url``, ``token_url``, ``client_id``,
    ``client_secret`` and ``fleet_token`` configured Cua Cloud, which has
    ended: they are accepted so existing calls keep working, and ignored.
    """
    retired = [
        name
        for name, value in (
            ("fleet_base_url", fleet_base_url),
            ("token_url", token_url),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("fleet_token", fleet_token),
        )
        if value is not None
    ]
    if retired:
        warnings.warn(
            f"configure({', '.join(f'{name}=' for name in retired)}) has no effect: "
            "Cua Cloud has ended",
            DeprecationWarning,
            stacklevel=2,
        )
    if api_key is not None:
        _global_config.api_key = api_key
    if base_url is not None:
        _global_config.base_url = base_url


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

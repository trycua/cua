"""Process-wide configuration for the legacy API-key helpers."""

from __future__ import annotations

import os
from dataclasses import dataclass
from typing import Optional

_DEFAULT_BASE_URL = "https://api.cua.ai"


@dataclass
class _Config:
    api_key: Optional[str] = None
    base_url: str = _DEFAULT_BASE_URL


_global_config = _Config()


def configure(*, api_key: Optional[str] = None, base_url: Optional[str] = None) -> None:
    """Set global configuration for the legacy ``login()`` and ``whoami()``
    helpers."""
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

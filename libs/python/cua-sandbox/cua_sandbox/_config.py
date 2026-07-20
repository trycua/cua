"""Global configuration for cloud sandbox provisioning."""

from __future__ import annotations

import os
from dataclasses import dataclass
from typing import Optional

_DEFAULT_BASE_URL = "https://run.cua.ai"
_DEFAULT_TOKEN_URL = "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token"


@dataclass
class _Config:
    api_key: Optional[str] = None
    base_url: str = _DEFAULT_BASE_URL
    token_url: str = _DEFAULT_TOKEN_URL
    client_id: Optional[str] = None
    client_secret: Optional[str] = None


_global_config = _Config()


def configure(
    *,
    api_key: Optional[str] = None,
    base_url: Optional[str] = None,
    token_url: Optional[str] = None,
    client_id: Optional[str] = None,
    client_secret: Optional[str] = None,
) -> None:
    """Set global configuration for cloud sandboxes.

    Fleet is the cloud backend. Its endpoint and token endpoint have defaults;
    callers normally need only an OAuth client ID and secret.
    """
    if api_key is not None:
        _global_config.api_key = api_key
    if base_url is not None:
        _global_config.base_url = base_url
    if token_url is not None:
        _global_config.token_url = token_url
    if client_id is not None:
        _global_config.client_id = client_id
    if client_secret is not None:
        _global_config.client_secret = client_secret


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


def get_token_url() -> str:
    return os.environ.get("CUA_TOKEN_URL") or _global_config.token_url


def get_client_id(override: Optional[str] = None) -> Optional[str]:
    return override or _global_config.client_id or os.environ.get("CUA_CLIENT_ID")


def get_client_secret(override: Optional[str] = None) -> Optional[str]:
    return override or _global_config.client_secret or os.environ.get("CUA_CLIENT_SECRET")


def _read_credentials_key() -> Optional[str]:
    """Read a legacy API key from ``~/.cua/credentials`` when present."""
    credential_path = os.path.join(os.path.expanduser("~"), ".cua", "credentials")
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

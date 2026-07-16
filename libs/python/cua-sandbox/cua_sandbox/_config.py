"""Global configuration for legacy and fleet-backed cloud APIs.

Legacy auth priority: per-call > configure() > ~/.cua/credentials > environment.
"""

from __future__ import annotations

import os
from dataclasses import dataclass
from typing import Optional


@dataclass
class _Config:
    api_key: Optional[str] = None
    base_url: str = "https://api.cua.ai"
    sandbox_base_url: str = "https://run.cua.ai"
    sandbox_client_id: Optional[str] = None
    sandbox_client_secret: Optional[str] = None
    sandbox_token_url: str = "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token"


_global_config = _Config()


def configure(
    *,
    api_key: Optional[str] = None,
    base_url: Optional[str] = None,
    sandbox_base_url: Optional[str] = None,
    sandbox_client_id: Optional[str] = None,
    sandbox_client_secret: Optional[str] = None,
    sandbox_token_url: Optional[str] = None,
) -> None:
    """Set global configuration for the CUA SDK.

    Args:
        api_key: API key for cloud sandboxes.
        base_url: Base URL for the CUA cloud API.
        sandbox_base_url: Base URL for fleet-backed sandbox lifecycle and services.
        sandbox_client_id: Per-user cyclops-cs client ID (``ukey-...``).
        sandbox_client_secret: Secret paired with ``sandbox_client_id``.
        sandbox_token_url: OAuth client-credentials token endpoint.
    """
    if api_key is not None:
        _global_config.api_key = api_key
    if base_url is not None:
        _global_config.base_url = base_url
    if sandbox_base_url is not None:
        _global_config.sandbox_base_url = sandbox_base_url
    if sandbox_client_id is not None:
        _global_config.sandbox_client_id = sandbox_client_id
    if sandbox_client_secret is not None:
        _global_config.sandbox_client_secret = sandbox_client_secret
    if sandbox_token_url is not None:
        _global_config.sandbox_token_url = sandbox_token_url


def get_api_key(override: Optional[str] = None) -> Optional[str]:
    """Resolve API key with priority: override > configure() > credentials file > env."""
    if override:
        return override
    if _global_config.api_key:
        return _global_config.api_key
    # Try credentials file
    cred = _read_credentials_key()
    if cred:
        return cred
    return os.environ.get("CUA_API_KEY")


def get_base_url() -> str:
    return os.environ.get("CUA_BASE_URL") or _global_config.base_url


def get_sandbox_base_url() -> str:
    return os.environ.get("CUA_SANDBOX_BASE_URL") or _global_config.sandbox_base_url


def get_sandbox_client_credentials() -> tuple[Optional[str], Optional[str]]:
    return (
        os.environ.get("CUA_CLIENT_ID") or _global_config.sandbox_client_id,
        os.environ.get("CUA_CLIENT_SECRET") or _global_config.sandbox_client_secret,
    )


def get_sandbox_token_url() -> str:
    return os.environ.get("CUA_TOKEN_URL") or _global_config.sandbox_token_url


def _read_credentials_key() -> Optional[str]:
    """Read API key from ~/.cua/credentials if it exists."""
    cred_path = os.path.join(os.path.expanduser("~"), ".cua", "credentials")
    try:
        with open(cred_path) as f:
            for line in f:
                line = line.strip()
                if line.startswith("api_key="):
                    return line[len("api_key=") :]
                if line.startswith("api_key ="):
                    return line[len("api_key =") :].strip()
    except FileNotFoundError:
        pass
    return None

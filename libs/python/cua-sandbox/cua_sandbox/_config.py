"""Global configuration — configure(api_key, base_url).

Auth priority: per-call > configure() > ~/.cua/credentials > CUA_API_KEY env var.
"""

from __future__ import annotations

import os
from dataclasses import dataclass, field
from typing import Optional


@dataclass
class _Config:
    api_key: Optional[str] = None
    base_url: str = "https://api.trycua.com"


_global_config = _Config()


def configure(
    *,
    api_key: Optional[str] = None,
    base_url: Optional[str] = None,
) -> None:
    """Set global configuration for the CUA SDK.

    Args:
        api_key: API key for cloud sandboxes.
        base_url: Base URL for the CUA cloud API.
    """
    if api_key is not None:
        _global_config.api_key = api_key
    if base_url is not None:
        _global_config.base_url = base_url


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
    return _global_config.base_url


def _read_credentials_key() -> Optional[str]:
    """Read API key from ~/.cua/credentials if it exists."""
    cred_path = os.path.join(os.path.expanduser("~"), ".cua", "credentials")
    try:
        with open(cred_path) as f:
            for line in f:
                line = line.strip()
                if line.startswith("api_key="):
                    return line[len("api_key="):]
                if line.startswith("api_key ="):
                    return line[len("api_key ="):].strip()
    except FileNotFoundError:
        pass
    return None

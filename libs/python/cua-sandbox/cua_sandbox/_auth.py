"""Authentication — login(), whoami(), credential storage.

login() opens a Clerk browser redirect and stores the resulting token
in ~/.cua/credentials. OAuth device flow deferred to a later stage.
"""

from __future__ import annotations

import json
import os
import time
import webbrowser
from pathlib import Path
from typing import Any, Dict, Optional

import httpx
from cua_sandbox._config import get_api_key, get_base_url

_CUA_DIR = Path.home() / ".cua"
_CREDENTIALS_FILE = _CUA_DIR / "credentials"


def login(*, base_url: Optional[str] = None) -> None:
    """Open the CUA login page in a browser and store credentials.

    This initiates a Clerk-based browser authentication flow.
    The user completes login in their browser, and the resulting
    API key is stored in ~/.cua/credentials.
    """
    url = base_url or get_base_url()
    login_url = f"{url}/auth/login"
    print(f"Opening {login_url} in your browser...")
    webbrowser.open(login_url)
    print("Complete the login in your browser.")
    print("Then paste the API key you receive below.")
    api_key = input("API key: ").strip()
    if not api_key:
        print("No API key provided. Aborting.")
        return
    _save_credentials(api_key=api_key)
    print("Credentials saved to ~/.cua/credentials")


def whoami(*, api_key: Optional[str] = None) -> Dict[str, Any]:
    """Return info about the authenticated user.

    Returns:
        Dict with user info (id, email, etc.) from the CUA API.
    """
    key = get_api_key(api_key)
    if not key:
        raise RuntimeError("Not authenticated. Run cua_sandbox.login() or set CUA_API_KEY.")
    resp = httpx.get(
        f"{get_base_url()}/v1/whoami",
        headers={"Authorization": f"Bearer {key}"},
        timeout=10,
    )
    resp.raise_for_status()
    return resp.json()


def _save_credentials(*, api_key: str) -> None:
    """Write credentials to ~/.cua/credentials."""
    _CUA_DIR.mkdir(parents=True, exist_ok=True)
    _CREDENTIALS_FILE.write_text(f"api_key={api_key}\n")
    # Restrict permissions on Unix
    try:
        _CREDENTIALS_FILE.chmod(0o600)
    except OSError:
        pass

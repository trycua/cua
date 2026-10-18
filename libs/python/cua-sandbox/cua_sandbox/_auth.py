"""Authentication — login(), whoami(), credential storage.

login() opens a Clerk browser redirect and stores the resulting token
in ~/.cua/credentials. OAuth device flow deferred to a later stage.
"""

from __future__ import annotations

import webbrowser
from pathlib import Path
from typing import Any, Dict, Optional

import httpx
from cua_sandbox._config import FLEET_CREDENTIALS_MISSING, get_api_key, get_base_url
from cua_sandbox._paths import cua_home, patched_or

_CUA_DIR = cua_home()
_CUA_DIR_DEFAULT = _CUA_DIR


def _cua_dir() -> Path:
    return patched_or(_CUA_DIR, _CUA_DIR_DEFAULT)


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
        raise RuntimeError(FLEET_CREDENTIALS_MISSING)
    resp = httpx.get(
        f"{get_base_url()}/v1/whoami",
        headers={"Authorization": f"Bearer {key}"},
        timeout=10,
    )
    resp.raise_for_status()
    return resp.json()


def _save_credentials(*, api_key: str) -> None:
    """Write credentials to ~/.cua/credentials."""
    cua_dir = _cua_dir()
    cua_dir.mkdir(parents=True, exist_ok=True)
    credentials = cua_dir / "credentials"
    credentials.write_text(f"api_key={api_key}\n")
    # Restrict permissions on Unix
    try:
        credentials.chmod(0o600)
    except OSError:
        pass

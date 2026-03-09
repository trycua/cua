"""SQLite-based credential storage for Cua CLI."""

import os
import sqlite3
from pathlib import Path
from typing import Optional

# Default storage location
CUA_DIR = Path.home() / ".cua"
CREDENTIALS_DB = CUA_DIR / "credentials.db"

# Key names
API_KEY_NAME = "api_key"

# Workspace key patterns
ACTIVE_WORKSPACE_KEY = "active_workspace"


class CredentialStore:
    """SQLite-based credential store with WAL mode for concurrent access."""

    def __init__(self, db_path: Path | None = None):
        """Initialize the credential store.

        Args:
            db_path: Path to the SQLite database. Defaults to ~/.cua/credentials.db
        """
        self.db_path = db_path or CREDENTIALS_DB
        self._ensure_db()

    def _ensure_db(self) -> None:
        """Ensure the database and table exist."""
        self.db_path.parent.mkdir(parents=True, exist_ok=True)

        conn = sqlite3.connect(self.db_path)
        try:
            # Enable WAL mode for better concurrent access
            conn.execute("PRAGMA journal_mode=WAL")

            # Create key-value table
            conn.execute(
                """
                CREATE TABLE IF NOT EXISTS kv (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                )
            """
            )
            conn.commit()
        finally:
            conn.close()

    def get(self, key: str) -> Optional[str]:
        """Get a value from the store.

        Args:
            key: The key to look up

        Returns:
            The value, or None if not found
        """
        conn = sqlite3.connect(self.db_path)
        try:
            cursor = conn.execute("SELECT value FROM kv WHERE key = ?", (key,))
            row = cursor.fetchone()
            return row[0] if row else None
        finally:
            conn.close()

    def set(self, key: str, value: str) -> None:
        """Set a value in the store.

        Args:
            key: The key to set
            value: The value to store
        """
        conn = sqlite3.connect(self.db_path)
        try:
            conn.execute(
                "INSERT OR REPLACE INTO kv (key, value) VALUES (?, ?)",
                (key, value),
            )
            conn.commit()
        finally:
            conn.close()

    def delete(self, key: str) -> bool:
        """Delete a value from the store.

        Args:
            key: The key to delete

        Returns:
            True if the key was deleted, False if it didn't exist
        """
        conn = sqlite3.connect(self.db_path)
        try:
            cursor = conn.execute("DELETE FROM kv WHERE key = ?", (key,))
            conn.commit()
            return cursor.rowcount > 0
        finally:
            conn.close()

    def delete_by_prefix(self, prefix: str) -> int:
        """Delete all keys matching a prefix.

        Args:
            prefix: The key prefix to match (uses LIKE prefix%)

        Returns:
            Number of rows deleted
        """
        conn = sqlite3.connect(self.db_path)
        try:
            cursor = conn.execute("DELETE FROM kv WHERE key LIKE ?", (prefix + "%",))
            conn.commit()
            return cursor.rowcount
        finally:
            conn.close()

    def list_by_prefix(self, prefix: str) -> list[tuple[str, str]]:
        """List all key-value pairs matching a prefix.

        Args:
            prefix: The key prefix to match (uses LIKE prefix%)

        Returns:
            List of (key, value) tuples
        """
        conn = sqlite3.connect(self.db_path)
        try:
            cursor = conn.execute("SELECT key, value FROM kv WHERE key LIKE ?", (prefix + "%",))
            return cursor.fetchall()
        finally:
            conn.close()

    def clear(self) -> None:
        """Clear all stored credentials."""
        conn = sqlite3.connect(self.db_path)
        try:
            conn.execute("DELETE FROM kv")
            conn.commit()
        finally:
            conn.close()


# Module-level convenience functions
_store: Optional[CredentialStore] = None


def _get_store() -> CredentialStore:
    """Get the global credential store instance."""
    global _store
    if _store is None:
        _store = CredentialStore()
    return _store


def save_workspace(slug: str, api_key: str, name: str, org: str) -> None:
    """Save workspace credentials and metadata."""
    store = _get_store()
    store.set(f"workspace:{slug}:api_key", api_key)
    store.set(f"workspace:{slug}:name", name)
    store.set(f"workspace:{slug}:org", org)


def get_active_workspace() -> Optional[str]:
    """Get the slug of the currently active workspace."""
    return _get_store().get(ACTIVE_WORKSPACE_KEY)


def set_active_workspace(slug: str) -> None:
    """Set the active workspace by slug."""
    _get_store().set(ACTIVE_WORKSPACE_KEY, slug)


def list_workspaces() -> list[dict]:
    """List all authenticated workspaces.

    Returns:
        List of dicts with keys: slug, name, org, is_active
    """
    store = _get_store()
    active = store.get(ACTIVE_WORKSPACE_KEY)
    rows = store.list_by_prefix("workspace:")

    # Group by slug — extract from keys like workspace:<slug>:api_key
    slugs: dict[str, dict] = {}
    for key, value in rows:
        parts = key.split(":")
        if len(parts) != 3:
            continue
        slug = parts[1]
        field = parts[2]
        if slug not in slugs:
            slugs[slug] = {"slug": slug, "name": "", "org": "", "is_active": False}
        if field == "name":
            slugs[slug]["name"] = value
        elif field == "org":
            slugs[slug]["org"] = value

    # Only include workspaces that have an api_key stored
    result = []
    for key, _value in rows:
        parts = key.split(":")
        if len(parts) == 3 and parts[2] == "api_key" and parts[1] in slugs:
            ws = slugs[parts[1]]
            ws["is_active"] = ws["slug"] == active
            result.append(ws)

    return result


def get_workspace_api_key(slug: str) -> Optional[str]:
    """Get the API key for a specific workspace."""
    return _get_store().get(f"workspace:{slug}:api_key")


def delete_workspace(slug: str) -> None:
    """Delete all stored data for a workspace."""
    store = _get_store()
    store.delete(f"workspace:{slug}:api_key")
    store.delete(f"workspace:{slug}:name")
    store.delete(f"workspace:{slug}:org")


def clear_all_workspaces() -> None:
    """Delete all workspace keys and the active workspace marker."""
    store = _get_store()
    store.delete_by_prefix("workspace:")
    store.delete(ACTIVE_WORKSPACE_KEY)


def get_api_key() -> Optional[str]:
    """Get the stored API key.

    Priority:
    1. CUA_API_KEY environment variable
    2. Active workspace's API key
    3. Legacy 'api_key' key (backward compat)

    Returns:
        The API key, or None if not found
    """
    # Environment variable takes precedence
    env_key = os.environ.get("CUA_API_KEY")
    if env_key:
        return env_key

    # Active workspace
    store = _get_store()
    active_slug = store.get(ACTIVE_WORKSPACE_KEY)
    if active_slug:
        ws_key = store.get(f"workspace:{active_slug}:api_key")
        if ws_key:
            return ws_key

    # Legacy fallback
    return store.get(API_KEY_NAME)


def save_api_key(api_key: str) -> None:
    """Save an API key to the credential store (legacy path)."""
    _get_store().set(API_KEY_NAME, api_key)


def clear_credentials() -> None:
    """Clear all stored credentials including workspaces and legacy key."""
    store = _get_store()
    clear_all_workspaces()
    store.delete(API_KEY_NAME)


def require_api_key() -> str:
    """Get the API key, raising an error if not found.

    Returns:
        The API key

    Raises:
        RuntimeError: If no API key is configured
    """
    api_key = get_api_key()
    if not api_key:
        raise RuntimeError(
            "No API key configured. Run 'cua auth login' to authenticate, "
            "or set the CUA_API_KEY environment variable."
        )
    return api_key

"""SQLite-based credential storage for CUA CLI."""

import os
import sqlite3
from pathlib import Path
from typing import Optional

# Default storage location
CUA_DIR = Path.home() / ".cua"
CREDENTIALS_DB = CUA_DIR / "credentials.db"

# Key names
API_KEY_NAME = "api_key"


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


def get_api_key() -> Optional[str]:
    """Get the stored API key.

    First checks CUA_API_KEY environment variable, then falls back to stored credentials.

    Returns:
        The API key, or None if not found
    """
    # Environment variable takes precedence
    env_key = os.environ.get("CUA_API_KEY")
    if env_key:
        return env_key

    return _get_store().get(API_KEY_NAME)


def save_api_key(api_key: str) -> None:
    """Save an API key to the credential store.

    Args:
        api_key: The API key to save
    """
    _get_store().set(API_KEY_NAME, api_key)


def clear_credentials() -> None:
    """Clear all stored credentials."""
    _get_store().clear()


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

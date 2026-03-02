"""Tests for auth.store module."""

import sqlite3
import threading
from unittest.mock import patch

import pytest
from cua_cli.auth.store import (
    CredentialStore,
    clear_credentials,
    get_api_key,
    require_api_key,
    save_api_key,
)


class TestCredentialStore:
    """Tests for CredentialStore class."""

    def test_init_creates_database(self, tmp_path):
        """Test that initializing creates the database file."""
        db_path = tmp_path / "test.db"
        CredentialStore(db_path)

        assert db_path.exists()

    def test_init_creates_parent_directory(self, tmp_path):
        """Test that initializing creates parent directories."""
        db_path = tmp_path / "nested" / "dir" / "test.db"
        CredentialStore(db_path)

        assert db_path.parent.exists()
        assert db_path.exists()

    def test_set_and_get(self, tmp_path):
        """Test setting and getting a value."""
        store = CredentialStore(tmp_path / "test.db")

        store.set("test_key", "test_value")
        result = store.get("test_key")

        assert result == "test_value"

    def test_get_nonexistent_key(self, tmp_path):
        """Test getting a nonexistent key returns None."""
        store = CredentialStore(tmp_path / "test.db")

        result = store.get("nonexistent")

        assert result is None

    def test_set_overwrites_existing(self, tmp_path):
        """Test that setting an existing key overwrites the value."""
        store = CredentialStore(tmp_path / "test.db")

        store.set("key", "value1")
        store.set("key", "value2")
        result = store.get("key")

        assert result == "value2"

    def test_delete_existing_key(self, tmp_path):
        """Test deleting an existing key returns True."""
        store = CredentialStore(tmp_path / "test.db")

        store.set("key", "value")
        result = store.delete("key")

        assert result is True
        assert store.get("key") is None

    def test_delete_nonexistent_key(self, tmp_path):
        """Test deleting a nonexistent key returns False."""
        store = CredentialStore(tmp_path / "test.db")

        result = store.delete("nonexistent")

        assert result is False

    def test_clear_removes_all(self, tmp_path):
        """Test that clear removes all values."""
        store = CredentialStore(tmp_path / "test.db")

        store.set("key1", "value1")
        store.set("key2", "value2")
        store.clear()

        assert store.get("key1") is None
        assert store.get("key2") is None

    def test_wal_mode_enabled(self, tmp_path):
        """Test that WAL mode is enabled for concurrent access."""
        db_path = tmp_path / "test.db"
        CredentialStore(db_path)

        conn = sqlite3.connect(db_path)
        try:
            cursor = conn.execute("PRAGMA journal_mode")
            mode = cursor.fetchone()[0]
            assert mode.lower() == "wal"
        finally:
            conn.close()

    def test_concurrent_access(self, tmp_path):
        """Test that concurrent access works with WAL mode."""
        db_path = tmp_path / "test.db"
        store = CredentialStore(db_path)
        results = []
        errors = []

        def writer():
            try:
                for i in range(10):
                    store.set(f"key_{i}", f"value_{i}")
            except Exception as e:
                errors.append(e)

        def reader():
            try:
                for i in range(10):
                    result = store.get(f"key_{i}")
                    if result is not None:
                        results.append(result)
            except Exception as e:
                errors.append(e)

        threads = [
            threading.Thread(target=writer),
            threading.Thread(target=reader),
            threading.Thread(target=reader),
        ]

        for t in threads:
            t.start()
        for t in threads:
            t.join()

        assert len(errors) == 0


class TestGetApiKey:
    """Tests for get_api_key function."""

    def test_returns_env_var_first(self, monkeypatch, tmp_path):
        """Test that environment variable takes precedence."""
        # Set up a store with a different value
        with patch("cua_cli.auth.store.CREDENTIALS_DB", tmp_path / "test.db"):
            with patch("cua_cli.auth.store._store", None):
                monkeypatch.setenv("CUA_API_KEY", "env-key")
                save_api_key("stored-key")

                result = get_api_key()

                assert result == "env-key"

    def test_falls_back_to_stored(self, monkeypatch, tmp_path):
        """Test that stored key is used when env var not set."""
        with patch("cua_cli.auth.store.CREDENTIALS_DB", tmp_path / "test.db"):
            with patch("cua_cli.auth.store._store", None):
                monkeypatch.delenv("CUA_API_KEY", raising=False)
                save_api_key("stored-key")

                result = get_api_key()

                assert result == "stored-key"

    def test_returns_none_when_not_found(self, monkeypatch, tmp_path):
        """Test that None is returned when no key is found."""
        with patch("cua_cli.auth.store.CREDENTIALS_DB", tmp_path / "test.db"):
            with patch("cua_cli.auth.store._store", None):
                monkeypatch.delenv("CUA_API_KEY", raising=False)

                result = get_api_key()

                assert result is None


class TestSaveApiKey:
    """Tests for save_api_key function."""

    def test_saves_api_key(self, tmp_path, monkeypatch):
        """Test that API key is saved correctly."""
        with patch("cua_cli.auth.store.CREDENTIALS_DB", tmp_path / "test.db"):
            with patch("cua_cli.auth.store._store", None):
                monkeypatch.delenv("CUA_API_KEY", raising=False)

                save_api_key("my-api-key")
                result = get_api_key()

                assert result == "my-api-key"


class TestClearCredentials:
    """Tests for clear_credentials function."""

    def test_clears_all_credentials(self, tmp_path, monkeypatch):
        """Test that all credentials are cleared."""
        with patch("cua_cli.auth.store.CREDENTIALS_DB", tmp_path / "test.db"):
            with patch("cua_cli.auth.store._store", None):
                monkeypatch.delenv("CUA_API_KEY", raising=False)

                save_api_key("my-api-key")
                clear_credentials()
                result = get_api_key()

                assert result is None


class TestRequireApiKey:
    """Tests for require_api_key function."""

    def test_returns_key_when_available(self, monkeypatch):
        """Test that key is returned when available."""
        monkeypatch.setenv("CUA_API_KEY", "my-api-key")

        result = require_api_key()

        assert result == "my-api-key"

    def test_raises_when_not_available(self, monkeypatch, tmp_path):
        """Test that RuntimeError is raised when no key available."""
        with patch("cua_cli.auth.store.CREDENTIALS_DB", tmp_path / "test.db"):
            with patch("cua_cli.auth.store._store", None):
                monkeypatch.delenv("CUA_API_KEY", raising=False)

                with pytest.raises(RuntimeError) as exc_info:
                    require_api_key()

                assert "No API key configured" in str(exc_info.value)
                assert "cua auth login" in str(exc_info.value)

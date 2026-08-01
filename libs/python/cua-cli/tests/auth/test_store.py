"""Tests for OS-backed OAuth credential storage."""

import json
from datetime import UTC, datetime
from unittest.mock import patch

import pytest
from cua_cli.auth.store import (
    CredentialStorageError,
    OAuthCredentials,
    clear_credentials,
    load_credentials,
    save_credentials,
)
from keyring.errors import NoKeyringError


def credentials() -> OAuthCredentials:
    return OAuthCredentials(
        access_token="access-token",
        refresh_token="refresh-token",
        expires_at=datetime(2030, 1, 1, tzinfo=UTC),
        scope="openid profile offline_access",
    )


def test_save_credentials_uses_keyring() -> None:
    with patch("cua_cli.auth.store.keyring.set_password") as set_password:
        save_credentials(credentials())

    service, account, raw = set_password.call_args.args
    assert (service, account) == ("run.cua.ai", "cua-cli")
    assert json.loads(raw)["access_token"] == "access-token"


def test_load_credentials_deserializes_keyring_value() -> None:
    raw = json.dumps(credentials().to_dict())
    with patch("cua_cli.auth.store.keyring.get_password", return_value=raw):
        loaded = load_credentials()

    assert loaded == credentials()


def test_clear_credentials_deletes_keyring_value() -> None:
    with (
        patch("cua_cli.auth.store.keyring.get_password", return_value="stored"),
        patch("cua_cli.auth.store.keyring.delete_password") as delete_password,
    ):
        assert clear_credentials() is True

    delete_password.assert_called_once_with("run.cua.ai", "cua-cli")


def test_store_errors_explain_secure_storage_requirement() -> None:
    with patch("cua_cli.auth.store.keyring.get_password", side_effect=NoKeyringError):
        with pytest.raises(CredentialStorageError, match="secure credential store"):
            load_credentials()

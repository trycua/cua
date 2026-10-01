"""Tests for OS-backed OAuth credential storage."""

import json
from datetime import UTC, datetime
from unittest.mock import patch

import pytest
from cua_cli.auth.store import (
    CredentialStorageError,
    OAuthCredentials,
    check_credential_store,
    clear_credentials,
    load_credentials,
    save_credentials,
)
from keyring.backends.fail import Keyring as FailKeyring
from keyring.backends.null import Keyring as NullKeyring
from keyring.errors import NoKeyringError


def credentials() -> OAuthCredentials:
    return OAuthCredentials(
        access_token="access-token",
        refresh_token="refresh-token",
        expires_at=datetime(2030, 1, 1, tzinfo=UTC),
        scope="openid profile offline_access",
    )


@pytest.mark.parametrize("raw", [None, "malformed old credentials", "{}"])
def test_preflight_reads_without_parsing_or_writing(raw) -> None:
    with (
        patch("cua_cli.auth.store.keyring.get_keyring", return_value=object()),
        patch("cua_cli.auth.store.keyring.get_password", return_value=raw) as read,
        patch("cua_cli.auth.store.keyring.set_password") as write,
        patch("cua_cli.auth.store.keyring.delete_password") as delete,
    ):
        check_credential_store()
    read.assert_called_once_with("run.cua.ai", "cua-cli")
    write.assert_not_called()
    delete.assert_not_called()


@pytest.mark.parametrize("backend", [FailKeyring(), NullKeyring()])
def test_preflight_rejects_unavailable_or_disabled_backend(backend) -> None:
    with (
        patch("cua_cli.auth.store.keyring.get_keyring", return_value=backend),
        patch("cua_cli.auth.store.keyring.get_password", side_effect=backend.get_password),
        patch("cua_cli.auth.store.keyring.set_password") as write,
    ):
        with pytest.raises(CredentialStorageError, match="secure credential store"):
            check_credential_store()
    write.assert_not_called()


@pytest.mark.parametrize("error", [NoKeyringError("secret-value"), OSError("secret-value")])
def test_preflight_sanitizes_backend_errors(error) -> None:
    with (
        patch("cua_cli.auth.store.keyring.get_keyring", return_value=object()),
        patch("cua_cli.auth.store.keyring.get_password", side_effect=error),
    ):
        with pytest.raises(CredentialStorageError) as caught:
            check_credential_store()
    assert "secret-value" not in str(caught.value)


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


def test_store_errors_name_a_headless_workaround() -> None:
    """A headless host has no OS keyring, so "configure an OS keyring" is a dead
    end. The error has to name something the user can actually run."""
    for target in ("get_password", "set_password"):
        with patch(f"cua_cli.auth.store.keyring.{target}", side_effect=NoKeyringError):
            with pytest.raises(CredentialStorageError) as excinfo:
                if target == "get_password":
                    load_credentials()
                else:
                    save_credentials(credentials())
            message = str(excinfo.value)
            assert "PYTHON_KEYRING_BACKEND" in message
            assert "FLEETS_TOKEN" in message
            # The encrypted option must be the one that works from a single
            # install. keyrings.alt's EncryptedKeyring needs pycryptodome on top
            # and dies with "No module named 'Crypto'" without it, which would
            # send the reader to a second dead end.
            assert "keyrings.cryptfile" in message
            assert "keyrings.alt.file.EncryptedKeyring" not in message


def _no_store_message() -> str:
    with patch("cua_cli.auth.store.keyring.get_password", side_effect=NoKeyringError):
        with pytest.raises(CredentialStorageError) as excinfo:
            load_credentials()
    return str(excinfo.value)


def test_unattended_hosts_are_told_to_use_a_token() -> None:
    """An encrypted keyring prompts for a passphrase on every command, so CI and
    containers cannot use one. The token path has to be named first."""
    message = _no_store_message()
    assert message.index("FLEETS_TOKEN") < message.index("keyrings.cryptfile")


def test_plaintext_option_is_marked_as_recoverable() -> None:
    message = _no_store_message()
    assert "PlaintextKeyring" in message
    assert "readable" in message

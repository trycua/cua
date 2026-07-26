"""Secure credential storage for Cua CLI."""

import json
from dataclasses import asdict, dataclass
from datetime import UTC, datetime
from typing import Any

import keyring
from keyring.errors import KeyringError, PasswordDeleteError

KEYRING_SERVICE = "run.cua.ai"
KEYRING_ACCOUNT = "cua-cli"


class CredentialStorageError(RuntimeError):
    """Raised when the operating system credential vault is unavailable."""


@dataclass(frozen=True)
class OAuthCredentials:
    """OIDC tokens issued to the CLI's public client."""

    access_token: str
    refresh_token: str | None
    expires_at: datetime
    token_type: str = "Bearer"
    scope: str | None = None

    @classmethod
    def from_dict(cls, value: dict[str, Any]) -> "OAuthCredentials":
        try:
            expires_at = datetime.fromisoformat(value["expires_at"])
            if expires_at.tzinfo is None:
                raise ValueError("expires_at must include a timezone")
            return cls(
                access_token=str(value["access_token"]),
                refresh_token=value.get("refresh_token"),
                expires_at=expires_at.astimezone(UTC),
                token_type=str(value.get("token_type", "Bearer")),
                scope=value.get("scope"),
            )
        except (KeyError, TypeError, ValueError) as error:
            raise CredentialStorageError(
                "Stored Cua credentials are invalid; log in again."
            ) from error

    def to_dict(self) -> dict[str, Any]:
        value = asdict(self)
        value["expires_at"] = self.expires_at.astimezone(UTC).isoformat()
        return value


def _read() -> str | None:
    try:
        return keyring.get_password(KEYRING_SERVICE, KEYRING_ACCOUNT)
    except KeyringError as error:
        raise CredentialStorageError(
            "No secure credential store is available. Configure an OS keyring before logging in."
        ) from error


def load_credentials() -> OAuthCredentials | None:
    """Load OIDC credentials from the operating system credential vault."""
    raw = _read()
    if raw is None:
        return None
    try:
        value = json.loads(raw)
    except json.JSONDecodeError as error:
        raise CredentialStorageError("Stored Cua credentials are invalid; log in again.") from error
    if not isinstance(value, dict):
        raise CredentialStorageError("Stored Cua credentials are invalid; log in again.")
    return OAuthCredentials.from_dict(value)


def save_credentials(credentials: OAuthCredentials) -> None:
    """Store OIDC credentials in the operating system credential vault."""
    try:
        keyring.set_password(KEYRING_SERVICE, KEYRING_ACCOUNT, json.dumps(credentials.to_dict()))
    except KeyringError as error:
        raise CredentialStorageError(
            "No secure credential store is available. Configure an OS keyring before logging in."
        ) from error


def clear_credentials() -> bool:
    """Remove locally stored OIDC credentials, returning whether any existed."""
    if _read() is None:
        return False
    try:
        keyring.delete_password(KEYRING_SERVICE, KEYRING_ACCOUNT)
    except PasswordDeleteError:
        return False
    except KeyringError as error:
        raise CredentialStorageError(
            "Could not remove credentials from the operating system credential store."
        ) from error
    return True

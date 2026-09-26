"""Credential interface shared by the OrcaRouter API-key and PKCE adapters.

Both authentication choices end in the same downstream artifact: a normal
OrcaRouter API key (``sk-orca-…``). The provider requests, the model catalog and
every AI entry point therefore depend only on this interface, never on how the
key was obtained.
"""

from __future__ import annotations

import hmac
import os
from dataclasses import dataclass
from typing import Literal, Protocol

from .origins import ORCA_KEY_ENV

CredentialSource = Literal["api_key", "pkce"]

#: Recorded next to the key so the two adapters can both read one secret slot
#: while still reporting honestly which choice produced it.
ORCA_KEY_SOURCE_ENV = "ORCA_KEY_SOURCE"

#: Marker used instead of the secret whenever a credential is rendered.
REDACTED = "sk-orca-…redacted"

_SECRET_PREFIX_LENGTH = 8


class OrcaRouterCredentialError(RuntimeError):
    """Raised when no usable OrcaRouter credential can be produced."""


def redact(value: str | None) -> str:
    """Return a log-safe rendering of a credential.

    Never returns enough material to reconstruct the key: only the fixed
    ``sk-orca-`` prefix is kept.
    """
    if not value:
        return ""
    if len(value) <= _SECRET_PREFIX_LENGTH:
        return REDACTED
    return f"{value[:_SECRET_PREFIX_LENGTH]}…redacted"


@dataclass(frozen=True)
class OrcaRouterCredential:
    """A resolved OrcaRouter credential and where it came from."""

    api_key: str
    source: CredentialSource
    #: Identifier of the account the key belongs to, when the flow exposed one.
    account_id: str | None = None
    #: Scope the authorization server actually granted (PKCE flows only).
    granted_scope: str | None = None
    #: Monotonic generation, used so a late failure cannot poison a newer login.
    generation: int = 0

    def __repr__(self) -> str:  # pragma: no cover - defensive redaction
        return (
            f"OrcaRouterCredential(api_key={redact(self.api_key)!r}, "
            f"source={self.source!r}, account_id={self.account_id!r}, "
            f"granted_scope={self.granted_scope!r}, generation={self.generation})"
        )

    def __str__(self) -> str:  # pragma: no cover - defensive redaction
        return repr(self)


class CredentialProvider(Protocol):
    """The seam both authentication choices implement."""

    source: CredentialSource

    def resolve(self) -> OrcaRouterCredential | None:
        """Return the current credential, or ``None`` when unconfigured."""
        ...

    def store(self, credential: OrcaRouterCredential) -> None:
        """Persist the credential in the project's existing secret storage."""
        ...

    def clear(self) -> bool:
        """Remove the stored credential, returning whether one existed."""
        ...


def looks_like_orca_key(value: str) -> bool:
    """Lightweight shape check.

    An ``sk-orca-`` prefix is not proof that a credential is valid; the first
    real request establishes validity.
    """
    return value.strip().startswith("sk-orca-")


class _EnvBackedProvider:
    """Shared ``ORCA_KEY`` handling for both adapters.

    The two choices read and write the same secret slot — the environment
    variable the project already populates from ``.env`` — so a key obtained by
    either choice is immediately usable by the other and by inference.
    ``ORCA_KEY_SOURCE`` records which choice produced the current value.
    """

    source: CredentialSource = "api_key"
    _account_id: str | None = None
    _granted_scope: str | None = None

    def __init__(self, environ: os._Environ[str] | dict[str, str] | None = None) -> None:
        self._environ = os.environ if environ is None else environ

    def _recorded_source(self) -> CredentialSource:
        recorded = (self._environ.get(ORCA_KEY_SOURCE_ENV) or "").strip()
        if recorded == "pkce":
            return "pkce"
        if recorded == "api_key":
            return "api_key"
        return self.source

    def resolve(self) -> OrcaRouterCredential | None:
        raw = (self._environ.get(ORCA_KEY_ENV) or "").strip()
        if not raw:
            return None
        return OrcaRouterCredential(
            api_key=raw,
            source=self._recorded_source(),
            account_id=self._account_id,
            granted_scope=self._granted_scope,
        )

    def store(self, credential: OrcaRouterCredential) -> None:
        key = credential.api_key.strip()
        if not key:
            raise OrcaRouterCredentialError("Refusing to store an empty OrcaRouter API key.")
        self._environ[ORCA_KEY_ENV] = key
        self._environ[ORCA_KEY_SOURCE_ENV] = credential.source

    def clear(self) -> bool:
        existed = bool((self._environ.get(ORCA_KEY_ENV) or "").strip())
        self._environ.pop(ORCA_KEY_ENV, None)
        self._environ.pop(ORCA_KEY_SOURCE_ENV, None)
        return existed


class ApiKeyCredentialProvider(_EnvBackedProvider):
    """The pasted-API-key adapter.

    Reads ``ORCA_KEY`` from the environment, which the project already populates
    from its ``.env`` file (``dotenv.load_dotenv()`` in the Gradio app and CLI
    entry points), and writes back the same variable.
    """

    source: CredentialSource = "api_key"


def same_secret(left: str | None, right: str | None) -> bool:
    """Constant-time comparison used for OAuth ``state`` and stored keys."""
    if left is None or right is None:
        return False
    return hmac.compare_digest(left.encode(), right.encode())

"""Ephemeral, one-shot credential delivery for production agent harnesses.

Credential leases deliberately do not implement a persistence format.  Their
only serializable surface is secret-free metadata suitable for audit events.
"""

from __future__ import annotations

import math
import re
import threading
import time
import uuid
from collections.abc import Callable, Mapping
from types import MappingProxyType
from typing import Any, NoReturn


MAX_SECRET_BYTES = 16_384
MAX_LEASE_SECRET_BYTES = 32_768

_POLICY_DIGEST = re.compile(r"^sha256:[0-9a-f]{64}$")
_APPROVED_CREDENTIALS: Mapping[tuple[str, str], frozenset[str]] = MappingProxyType(
    {
        ("openai", "codex"): frozenset({"CDB_CODEX_AUTH_JSON", "OPENAI_API_KEY"}),
        ("openai", "opencode"): frozenset({"OPENAI_API_KEY"}),
        ("anthropic", "claude-code"): frozenset({"ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"}),
        ("anthropic", "opencode"): frozenset({"ANTHROPIC_API_KEY"}),
    }
)


class CredentialLeaseError(ValueError):
    """A credential lease was invalid or could not be consumed safely."""


class CredentialLease:
    """Hold approved environment credentials until one bound consumption.

    ``consume`` transfers decoded credential values to its caller and scrubs
    every mutable internal buffer before returning.  Python strings returned
    to the caller cannot themselves be overwritten, so the caller must keep
    the resulting environment mapping process-local and short-lived.
    """

    __slots__ = (
        "_buffers",
        "_clock",
        "_expires_at",
        "_harness",
        "_lease_id",
        "_lock",
        "_policy_digest",
        "_provider",
        "_state",
    )
    __hash__ = None

    def __init__(
        self,
        *,
        provider: str,
        harness: str,
        credentials: Mapping[str, str | bytes | bytearray],
        expires_at: float,
        policy_digest: str,
        clock: Callable[[], float] = time.time,
    ) -> None:
        if not isinstance(provider, str) or not isinstance(harness, str):
            raise CredentialLeaseError("credential provider and harness must be strings")
        approved = _APPROVED_CREDENTIALS.get((provider, harness))
        if approved is None:
            raise CredentialLeaseError("unsupported credential provider/harness binding")
        if not isinstance(credentials, Mapping) or not credentials:
            raise CredentialLeaseError("credential environment must be a non-empty mapping")
        names = set(credentials)
        if any(not isinstance(name, str) for name in names) or not names <= approved:
            raise CredentialLeaseError(
                "credential environment contains an unsupported variable name"
            )
        if (
            not isinstance(expires_at, (int, float))
            or isinstance(expires_at, bool)
            or not math.isfinite(expires_at)
        ):
            raise CredentialLeaseError("credential lease expiry must be finite")
        if not isinstance(policy_digest, str) or not _POLICY_DIGEST.fullmatch(policy_digest):
            raise CredentialLeaseError("credential lease policy digest is malformed")
        if not callable(clock):
            raise CredentialLeaseError("credential lease clock must be callable")
        buffers: dict[str, bytearray] = {}
        total = 0
        try:
            for name, value in credentials.items():
                encoded = self._encode_secret(value)
                total += len(encoded)
                if total > MAX_LEASE_SECRET_BYTES:
                    self._scrub(buffers)
                    self._scrub_one(encoded)
                    raise CredentialLeaseError(
                        "credential environment exceeds the aggregate size limit"
                    )
                buffers[name] = encoded
        except BaseException:
            self._scrub(buffers)
            raise

        self._provider = provider
        self._harness = harness
        self._expires_at = float(expires_at)
        self._policy_digest = policy_digest
        self._lease_id = str(uuid.uuid4())
        self._clock = clock
        self._buffers = buffers
        self._state = "active"
        self._lock = threading.Lock()

    @staticmethod
    def _encode_secret(value: str | bytes | bytearray) -> bytearray:
        if isinstance(value, str):
            # UTF-8 is at least one byte per code point, so this cheap check
            # prevents an already-oversized string from causing a large copy.
            if len(value) > MAX_SECRET_BYTES:
                raise CredentialLeaseError("credential value exceeds the size limit")
            try:
                encoded = bytearray(value, "utf-8")
            except UnicodeError as error:
                raise CredentialLeaseError("credential value is not valid UTF-8") from error
        elif isinstance(value, (bytes, bytearray)):
            if len(value) > MAX_SECRET_BYTES:
                raise CredentialLeaseError("credential value exceeds the size limit")
            encoded = bytearray(value)
            try:
                encoded.decode("utf-8")
            except UnicodeError as error:
                CredentialLease._scrub_one(encoded)
                raise CredentialLeaseError("credential value is not valid UTF-8") from error
        else:
            raise CredentialLeaseError("credential value must be text or bytes")
        if not encoded:
            raise CredentialLeaseError("credential value must not be empty")
        if len(encoded) > MAX_SECRET_BYTES:
            CredentialLease._scrub_one(encoded)
            raise CredentialLeaseError("credential value exceeds the size limit")
        if 0 in encoded:
            CredentialLease._scrub_one(encoded)
            raise CredentialLeaseError("credential value contains a NUL byte")
        return encoded

    @staticmethod
    def _scrub_one(buffer: bytearray) -> None:
        for index in range(len(buffer)):
            buffer[index] = 0

    @classmethod
    def _scrub(cls, buffers: Mapping[str, bytearray]) -> None:
        for buffer in buffers.values():
            cls._scrub_one(buffer)

    @property
    def provider(self) -> str:
        return self._provider

    @property
    def harness(self) -> str:
        return self._harness

    @property
    def state(self) -> str:
        with self._lock:
            self._expire_if_needed()
            return self._state

    def metadata(self) -> dict[str, str | float]:
        """Return the complete, intentionally secret-free audit surface."""

        with self._lock:
            self._expire_if_needed()
            return {
                "lease_id": self._lease_id,
                "provider": self._provider,
                "expires_at": self._expires_at,
                "state": self._state,
                "policy_digest": self._policy_digest,
            }

    def consume(self, *, provider: str, harness: str) -> dict[str, str]:
        """Consume once at the explicitly matching provider/harness boundary."""

        with self._lock:
            self._expire_if_needed()
            if self._state != "active":
                raise CredentialLeaseError("credential lease is not active")
            if provider != self._provider or harness != self._harness:
                self._destroy_locked("destroyed")
                raise CredentialLeaseError("credential lease binding mismatch")

            environment: dict[str, str] = {}
            try:
                environment = {
                    name: buffer.decode("utf-8") for name, buffer in self._buffers.items()
                }
            finally:
                self._destroy_locked("consumed")
            return environment

    def destroy(self) -> None:
        """Irrevocably scrub an unconsumed lease."""

        with self._lock:
            if self._state == "active":
                self._destroy_locked("destroyed")

    def _expire_if_needed(self) -> None:
        if self._state != "active":
            return
        try:
            now = self._clock()
            valid = (
                isinstance(now, (int, float)) and not isinstance(now, bool) and math.isfinite(now)
            )
        except Exception as error:
            self._destroy_locked("expired")
            raise CredentialLeaseError("credential lease clock failed") from error
        if not valid:
            self._destroy_locked("expired")
            raise CredentialLeaseError("credential lease clock returned an invalid value")
        if now >= self._expires_at:
            self._destroy_locked("expired")

    def _destroy_locked(self, state: str) -> None:
        self._scrub(self._buffers)
        self._buffers.clear()
        self._state = state

    def __repr__(self) -> str:
        metadata = self.metadata()
        return (
            "CredentialLease("
            f"lease_id={metadata['lease_id']!r}, provider={self._provider!r}, "
            f"harness={self._harness!r}, expires_at={self._expires_at!r}, "
            f"state={metadata['state']!r}, policy_digest={self._policy_digest!r})"
        )

    def __copy__(self) -> NoReturn:
        raise TypeError("credential leases cannot be copied")

    def __deepcopy__(self, memo: Any) -> NoReturn:
        raise TypeError("credential leases cannot be copied")

    def __reduce_ex__(self, protocol: int) -> NoReturn:
        raise TypeError("credential leases cannot be persisted")

    def __del__(self) -> None:
        # Best effort only: deterministic callers should use consume/destroy.
        try:
            self._scrub(self._buffers)
            self._buffers.clear()
        except (AttributeError, TypeError):
            pass

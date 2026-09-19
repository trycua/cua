"""Server-side OrcaRouter login session for the Gradio UI.

A local UI keeps one "login already in progress" lock. Every terminal path has
to release it — success, denial, exchange error, timeout, an explicit Cancel,
switching authentication method, closing the panel, unmount, and ``pagehide``.

``pagehide`` needs its own handling. When the browser puts the page into the
back-forward cache, the pending coroutine's guarded ``finally`` block correctly
refuses to mutate state, which would leave a restored page permanently busy.
:meth:`OrcaRouterConnectSession.pagehide` therefore clears the busy flag and the
authorization hint *synchronously* and asks for the server-side work to be
cancelled, without waiting for the in-flight task.
"""

from __future__ import annotations

import threading
from dataclasses import dataclass, field
from typing import Any

from .credentials import OrcaRouterCredential, redact
from .pkce import OrcaRouterAuthError, PkceAttempt, build_authorize_url


class OrcaRouterLoginBusy(RuntimeError):
    """Raised when a login is already in progress for this session."""


@dataclass
class ConnectSnapshot:
    """UI-visible login state. Never contains the verifier or the key."""

    busy: bool
    generation: int
    hint: str
    url: str | None
    message: str | None
    ok: bool
    masked: str


@dataclass
class OrcaRouterConnectSession:
    """A generation-guarded connect lock shared by the server and the UI."""

    provider: Any
    auth_base_url: str | None = None
    app_name: str = "Cua Agent"
    _generation: int = 0
    _busy: bool = False
    _hint: str = ""
    _url: str | None = None
    _message: str | None = None
    _ok: bool = False
    _masked: str = ""
    _cancelled: set[int] = field(default_factory=set)
    _lock: threading.Lock = field(default_factory=threading.Lock)
    #: The verifier for the in-flight attempt. Server-side only: it is never
    #: handed to the browser, logged, or placed in a URL.
    _pending: Any = None

    # -- state -----------------------------------------------------------

    @property
    def generation(self) -> int:
        return self._generation

    @property
    def pending_attempt(self) -> PkceAttempt | None:
        """The verifier holder for the current attempt, or ``None``."""
        with self._lock:
            if not self._busy or self._pending is None:
                return None
            return self._pending  # type: ignore[no-any-return]

    def snapshot(self) -> ConnectSnapshot:
        return ConnectSnapshot(
            busy=self._busy,
            generation=self._generation,
            hint=self._hint,
            url=self._url,
            message=self._message,
            ok=self._ok,
            masked=self._masked,
        )

    def _is_current(self, generation: int) -> bool:
        return generation == self._generation

    # -- lifecycle -------------------------------------------------------

    def begin(self) -> tuple[int, PkceAttempt, str]:
        """Start an attempt, returning ``(generation, attempt, authorize_url)``.

        The verifier stays inside the returned attempt and is never handed to
        the browser.
        """
        with self._lock:
            if self._busy:
                raise OrcaRouterLoginBusy(
                    "An OrcaRouter sign-in is already in progress. Cancel it or wait "
                    "for it to finish."
                )
            self._generation += 1
            generation = self._generation
            self._busy = True
            self._message = None
            self._ok = False
            self._url = None
            self._cancelled.discard(generation)
        attempt = PkceAttempt.create()
        base = self.auth_base_url or self.provider.auth_base_url
        url = build_authorize_url(
            attempt,
            auth_base_url=base,
            # The Gradio server may run in a container or behind a hosted URL, so a
            # browser redirect to 127.0.0.1 is not always reachable: ask for the
            # out-of-band code, which also keeps S256 mandatory.
            callback_url="oob",
            app_name=self.app_name,
        )
        with self._lock:
            if not self._is_current(generation) or generation in self._cancelled:
                return generation, attempt, url
            self._pending = attempt
        self.deliver_url(generation, url)
        return generation, attempt, url

    def deliver_url(self, generation: int, url: str) -> bool:
        """Publish the authorization URL for the current generation only."""
        with self._lock:
            if not self._is_current(generation) or generation in self._cancelled:
                return False
            self._url = url
            self._hint = (
                "Authorize in the browser, then paste the code shown by OrcaRouter " "below."
            )
            return True

    def is_cancelled(self, generation: int) -> bool:
        with self._lock:
            return generation in self._cancelled or not self._is_current(generation)

    def succeed(self, generation: int, credential: OrcaRouterCredential) -> bool:
        """Record a successful exchange; ignored for a superseded generation."""
        with self._lock:
            if not self._is_current(generation) or generation in self._cancelled:
                return False
            self._busy = False
            self._pending = None
            self._hint = ""
            self._url = None
            self._ok = True
            self._message = f"Connected to OrcaRouter ({redact(credential.api_key)})."
            self._masked = redact(credential.api_key)
            return True

    def fail(self, generation: int, error: Any) -> bool:
        """Record a failure; ignored for a superseded generation."""
        with self._lock:
            if not self._is_current(generation):
                return False
            self._busy = False
            self._pending = None
            self._hint = ""
            self._url = None
            self._ok = False
            self._message = str(error)
            if isinstance(error, OrcaRouterAuthError) and error.kind == "denied":
                self._message = (
                    "Authorization was denied in the browser. Nothing was stored; you "
                    "can try again or paste an API key instead."
                )
            return True

    def cancel(self, *, reason: str = "cancelled") -> int:
        """Explicit cancel: invalidate the generation and release the lock."""
        with self._lock:
            if not self._busy:
                return self._generation
            self._cancelled.add(self._generation)
            self._busy = False
            self._pending = None
            self._hint = ""
            self._url = None
            self._ok = False
            self._message = "Authorization cancelled." if reason == "cancelled" else str(reason)
            return self._generation

    def pagehide(self, generation: int | None = None) -> int:
        """Back-forward cache path.

        Clears busy/hint synchronously instead of relying on the in-flight
        task's guarded ``finally`` block, then asks the server to cancel. A
        second login can start immediately without remounting the page.
        """
        with self._lock:
            self._generation += 1
            target = generation if generation is not None else self._generation
            self._cancelled.add(target)
            self._busy = False
            self._pending = None
            self._hint = ""
            self._url = None
            self._ok = False
            self._message = "The page was hidden; the pending authorization was cancelled."
            return self._generation

    def switch_method(self, *, reason: str = "Switched authentication method.") -> int:
        """Switching between API key and Connect must release the lock too."""
        return self.cancel(reason=reason)

    def reset(self) -> None:
        """Clear any terminal message, e.g. when the panel is reopened."""
        with self._lock:
            if self._busy:
                return
            self._message = None
            self._ok = False

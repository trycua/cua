"""OAuth 2.0 + PKCE connect flow for OrcaRouter.

Two flows are implemented, both S256-only:

* **Flow A — loopback redirect** (``connect_via_loopback``): a listener is bound
  first so the port is known, the browser returns the code to it. A human can
  still choose "show me a code" on the consent screen, so S256 is mandatory here
  as well.
* **Flow B — out-of-band code** (``connect_via_out_of_band``): ``callback_url=oob``
  and the code is displayed on the consent screen for the user to paste back.

Flow C (device grant) is not implemented; it is optional and does not replace
PKCE.

The exchanged value is a durable OrcaRouter API key, not a refresh token: it is
reused until revoked, never refreshed, and never re-minted on every launch
(OrcaRouter allows 10 PKCE-issued keys per user per 24 hours).
"""

from __future__ import annotations

import base64
import hashlib
import hmac
import http.server
import json
import os
import secrets
import threading
import urllib.parse
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from typing import Any

from .credentials import (
    ORCA_KEY_SOURCE_ENV,
    CredentialProvider,
    OrcaRouterCredential,
    OrcaRouterCredentialError,
)
from .origins import ORCA_KEY_ENV, authorize_url, exchange_url

APP_NAME = "Cua Agent"
#: ``api`` is the scope the agent needs; ``connector`` is wider than we use.
REQUESTED_SCOPE = "api"
CHALLENGE_METHOD = "S256"

CODE_TTL_SECONDS = 600
DEFAULT_TIMEOUT_SECONDS = 300

_CALLBACK_PATH = "/cb"
_SUCCESS_PAGE = (
    b"<!doctype html><html><body><p>Connected to OrcaRouter. "
    b"You can close this tab.</p></body></html>"
)
_DENIED_PAGE = (
    b"<!doctype html><html><body><p>Authorization was denied. "
    b"You can close this tab.</p></body></html>"
)


class OrcaRouterAuthError(RuntimeError):
    """Raised for a failed, denied, expired or cancelled authorization."""

    def __init__(self, message: str, *, kind: str = "error") -> None:
        super().__init__(message)
        #: Machine-readable reason: denied / state_mismatch / expired / cancelled /
        #: exchange_rejected / rate_limited / network / protocol.
        self.kind = kind


def _b64url(raw: bytes) -> str:
    return base64.urlsafe_b64encode(raw).decode().rstrip("=")


def generate_verifier() -> str:
    """Return a fresh PKCE verifier from a cryptographic RNG."""
    return _b64url(secrets.token_bytes(32))


def generate_state() -> str:
    """Return a fresh opaque CSRF state from a cryptographic RNG."""
    return _b64url(secrets.token_bytes(16))


def code_challenge_for(verifier: str) -> str:
    """Return the unpadded ``base64url(sha256(verifier))`` challenge."""
    return _b64url(hashlib.sha256(verifier.encode()).digest())


@dataclass(frozen=True)
class PkceAttempt:
    """State for one authorization attempt. The verifier never leaves it."""

    verifier: str
    state: str

    @property
    def code_challenge(self) -> str:
        return code_challenge_for(self.verifier)

    @classmethod
    def create(cls) -> "PkceAttempt":
        return cls(verifier=generate_verifier(), state=generate_state())


def build_authorize_url(
    attempt: PkceAttempt,
    *,
    auth_base_url: str,
    callback_url: str,
    app_name: str = APP_NAME,
    scope: str = REQUESTED_SCOPE,
) -> str:
    """Build the consent-screen URL. Never the inference origin."""
    params = {
        "callback_url": callback_url,
        "code_challenge": attempt.code_challenge,
        "code_challenge_method": CHALLENGE_METHOD,
        "state": attempt.state,
        "app_name": app_name,
        "scope": scope,
    }
    return f"{authorize_url(auth_base_url)}?{urllib.parse.urlencode(params)}"


@dataclass(frozen=True)
class ExchangeResult:
    """The parsed exchange response."""

    api_key: str
    account_id: str | None
    granted_scope: str | None

    def to_credential(self, *, generation: int = 0) -> OrcaRouterCredential:
        return OrcaRouterCredential(
            api_key=self.api_key,
            source="pkce",
            account_id=self.account_id,
            granted_scope=self.granted_scope,
            generation=generation,
        )


def parse_exchange_payload(status: int, payload: Mapping[str, Any] | None) -> ExchangeResult:
    """Validate an exchange response body.

    ``scope`` in the response is what was *granted*, not what was requested; a
    narrower grant is accepted but reported so the caller can say so.
    """
    body = payload or {}
    if status == 403:
        raise OrcaRouterAuthError(
            "OrcaRouter rejected the authorization code: it is unknown, expired or "
            "already used, or the PKCE verifier did not match. Start a new connect "
            "attempt.",
            kind="exchange_rejected",
        )
    if status == 400:
        raise OrcaRouterAuthError(
            "OrcaRouter rejected the exchange request. This usually means the code "
            "challenge method did not match the one sent at authorize time.",
            kind="protocol",
        )
    if status == 429:
        raise OrcaRouterAuthError(
            "OrcaRouter rate-limited this request. A user may hold at most 10 "
            "PKCE-issued keys per 24 hours; wait before connecting again.",
            kind="rate_limited",
        )
    if status >= 500:
        raise OrcaRouterAuthError(
            f"OrcaRouter authorization service failed (HTTP {status}). Try again later.",
            kind="network",
        )
    if status != 200:
        raise OrcaRouterAuthError(
            f"OrcaRouter returned an unexpected response (HTTP {status}).", kind="protocol"
        )

    key = body.get("key")
    if not isinstance(key, str) or not key.strip():
        raise OrcaRouterAuthError(
            "OrcaRouter returned no API key in the exchange response.", kind="protocol"
        )
    account = body.get("user_id")
    scope = body.get("scope")
    return ExchangeResult(
        api_key=key.strip(),
        account_id=str(account) if account is not None else None,
        granted_scope=str(scope) if scope is not None else None,
    )


def post_exchange(
    url: str, attempt: PkceAttempt, code: str, *, timeout: float = 30.0
) -> ExchangeResult:
    """Exchange an auth code for a durable API key.

    Uses ``httpx`` (already a dependency of this package). The verifier is sent
    in the request body only — never in the URL.
    """
    import httpx

    body = {
        "code": code,
        "code_verifier": attempt.verifier,
        "code_challenge_method": CHALLENGE_METHOD,
    }
    try:
        response = httpx.post(url, json=body, timeout=timeout)
    except httpx.HTTPError as error:
        raise OrcaRouterAuthError(
            f"Could not reach the OrcaRouter authorization service: {error.__class__.__name__}.",
            kind="network",
        ) from None
    payload: Mapping[str, Any] | None
    try:
        parsed = response.json()
        payload = parsed if isinstance(parsed, dict) else None
    except ValueError:
        payload = None
    return parse_exchange_payload(response.status_code, payload)


def _extract_callback(query: Mapping[str, list[str]], expected_state: str) -> str:
    """Validate the loopback callback query and return the auth code.

    ``state`` is compared in constant time before anything else is trusted.
    """
    returned_state = (query.get("state") or [""])[0]
    if not hmac.compare_digest(returned_state, expected_state):
        raise OrcaRouterAuthError(
            "The authorization response did not match this attempt (state mismatch). "
            "The login was aborted; start a new connect attempt.",
            kind="state_mismatch",
        )
    error = (query.get("error") or [""])[0]
    if error:
        kind = "denied" if error == "access_denied" else "protocol"
        raise OrcaRouterAuthError(
            f"OrcaRouter authorization failed: {error}.",
            kind=kind,
        )
    code = (query.get("code") or [""])[0]
    if not code:
        raise OrcaRouterAuthError("The authorization response contained no code.", kind="protocol")
    return code


class _CallbackHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self) -> None:  # noqa: N802 - stdlib naming
        parsed = urllib.parse.urlsplit(self.path)
        if parsed.path != _CALLBACK_PATH:
            self.send_response(404)
            self.end_headers()
            return
        query = urllib.parse.parse_qs(parsed.query, keep_blank_values=True)
        try:
            code = _extract_callback(query, self.server.expected_state)  # type: ignore[attr-defined]
        except OrcaRouterAuthError as error:
            self._respond(_DENIED_PAGE, error)
            return
        self._respond(_SUCCESS_PAGE, None, code)

    def _respond(
        self, page: bytes, error: OrcaRouterAuthError | None, code: str | None = None
    ) -> None:
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(page)))
        self.end_headers()
        try:
            self.wfile.write(page)
        except OSError:
            pass
        server = self.server
        server.result_code = code  # type: ignore[attr-defined]
        server.result_error = error  # type: ignore[attr-defined]
        server.done.set()  # type: ignore[attr-defined]

    def log_message(self, *_args: Any) -> None:
        """Silence the default stderr access log; it would leak the query string."""


class LoopbackListener:
    """A one-shot ``127.0.0.1`` listener for Flow A."""

    def __init__(self) -> None:
        self._server = http.server.HTTPServer(("127.0.0.1", 0), _CallbackHandler)
        self._server.done = threading.Event()  # type: ignore[attr-defined]
        self._server.result_code = None  # type: ignore[attr-defined]
        self._server.result_error = None  # type: ignore[attr-defined]
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)
        self._closed = False

    @property
    def port(self) -> int:
        return int(self._server.server_address[1])

    @property
    def callback_url(self) -> str:
        return f"http://127.0.0.1:{self.port}{_CALLBACK_PATH}"

    def start(self, expected_state: str) -> None:
        self._server.expected_state = expected_state  # type: ignore[attr-defined]
        self._thread.start()

    def wait(self, timeout: float) -> str:
        if not self._server.done.wait(timeout):  # type: ignore[attr-defined]
            raise OrcaRouterAuthError(
                f"Timed out after {int(timeout)}s waiting for the browser to complete "
                f"the authorization. Start a new connect attempt.",
                kind="expired",
            )
        error = self._server.result_error  # type: ignore[attr-defined]
        if error is not None:
            raise error
        return str(self._server.result_code)  # type: ignore[attr-defined]

    def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        # `shutdown()` blocks until `serve_forever` returns, so it must only be
        # called once the serving thread actually started.
        if self._thread.is_alive():
            try:
                self._server.shutdown()
            except OSError:
                pass
        try:
            self._server.server_close()
        except OSError:
            pass
        if self._thread.is_alive():
            self._thread.join(timeout=2)

    def __enter__(self) -> "LoopbackListener":
        return self

    def __exit__(self, *_exc: object) -> None:
        self.close()


def connect_via_loopback(
    *,
    auth_base_url: str,
    attempt: PkceAttempt | None = None,
    app_name: str = APP_NAME,
    open_browser: Callable[[str], Any] | None = None,
    on_url: Callable[[str], None] | None = None,
    timeout: float = DEFAULT_TIMEOUT_SECONDS,
) -> OrcaRouterCredential:
    """Flow A: bind a loopback listener, open the browser, exchange the code."""
    import webbrowser

    attempt = attempt or PkceAttempt.create()
    listener = LoopbackListener()
    try:
        listener.start(attempt.state)
        url = build_authorize_url(
            attempt,
            auth_base_url=auth_base_url,
            callback_url=listener.callback_url,
            app_name=app_name,
        )
        if on_url is not None:
            on_url(url)
        opener = open_browser or (lambda target: webbrowser.open(target))
        try:
            opener(url)
        except Exception:  # noqa: BLE001 - a missing browser must not abort the flow
            pass
        code = listener.wait(timeout)
    finally:
        listener.close()
    result = post_exchange(exchange_url(auth_base_url), attempt, code)
    return result.to_credential()


def connect_via_out_of_band(
    *,
    auth_base_url: str,
    attempt: PkceAttempt | None = None,
    app_name: str = APP_NAME,
    open_browser: Callable[[str], Any] | None = None,
    on_url: Callable[[str], None] | None = None,
    read_code: Callable[[], str],
) -> OrcaRouterCredential:
    """Flow B: ``callback_url=oob`` and a pasted code. S256 is mandatory."""
    import webbrowser

    attempt = attempt or PkceAttempt.create()
    url = build_authorize_url(
        attempt, auth_base_url=auth_base_url, callback_url="oob", app_name=app_name
    )
    if on_url is not None:
        on_url(url)
    opener = open_browser or (lambda target: webbrowser.open(target))
    try:
        opener(url)
    except Exception:  # noqa: BLE001
        pass
    code = (read_code() or "").strip()
    if not code:
        raise OrcaRouterAuthError("No authorization code was provided.", kind="cancelled")
    result = post_exchange(exchange_url(auth_base_url), attempt, code)
    return result.to_credential()


#: Narrower-than-requested grants we still accept, and what they mean.
_ACCEPTED_SCOPES = frozenset({"api"})


def connect(
    *,
    auth_base_url: str,
    store: "OrcaRouterCredentialStore",
    read_code: Callable[[], str] | None = None,
    prefer_loopback: bool = False,
    on_url: Callable[[str], None] | None = None,
    app_name: str = APP_NAME,
    open_browser: Callable[[str], Any] | None = None,
) -> OrcaRouterCredential:
    """Run the connect flow and persist the resulting durable key.

    ``read_code`` selects Flow B; without it Flow A is used when
    ``prefer_loopback`` is set. The stored secret is only replaced after a
    successful exchange, so a failed attempt never destroys a working key.
    """
    if read_code is not None:
        credential = connect_via_out_of_band(
            auth_base_url=auth_base_url,
            app_name=app_name,
            open_browser=open_browser,
            on_url=on_url,
            read_code=read_code,
        )
    else:
        credential = connect_via_loopback(
            auth_base_url=auth_base_url,
            app_name=app_name,
            open_browser=open_browser,
            on_url=on_url,
        )
    if credential.granted_scope and credential.granted_scope not in _ACCEPTED_SCOPES:
        raise OrcaRouterAuthError(
            f"OrcaRouter granted the scope {credential.granted_scope!r}, which this "
            f"client does not use; expected {REQUESTED_SCOPE!r}. The authorization "
            f"was not stored.",
            kind="scope_downgrade",
        )
    store.save(credential)
    return credential


class OrcaRouterCredentialStore:
    """The PKCE adapter's persistence into the project's existing secret file.

    The key is written to the same ``.env`` file the project already reads
    through ``dotenv.load_dotenv()`` (gitignored). No new secret store is
    introduced. ``refresh`` stays a compatibility placeholder: OrcaRouter issues
    a durable key, so nothing here ever treats it as a refresh token.
    """

    source = "pkce"

    #: Populated when the flow exposed an account id or granted scope.
    _account_id: str | None = None
    _granted_scope: str | None = None

    def __init__(
        self, env_file: str | None = None, environ: Mapping[str, str] | None = None
    ) -> None:
        self._env_file = env_file
        self._environ = os.environ if environ is None else environ

    def _file(self) -> str:
        if self._env_file:
            return self._env_file
        from dotenv import find_dotenv

        found = find_dotenv(usecwd=True)
        return found or os.path.join(os.getcwd(), ".env")

    def resolve(self) -> OrcaRouterCredential | None:
        """Read the key this adapter persisted, marking it as PKCE-issued."""
        raw = (self._environ.get(ORCA_KEY_ENV) or "").strip()
        if not raw:
            return None
        recorded = (self._environ.get(ORCA_KEY_SOURCE_ENV) or "").strip()
        if recorded != "pkce":
            return None
        return OrcaRouterCredential(
            api_key=raw,
            source="pkce",
            account_id=self._account_id,
            granted_scope=self._granted_scope,
        )

    def save(self, credential: OrcaRouterCredential) -> None:
        from dotenv import set_key

        path = self._file()
        set_key(path, ORCA_KEY_ENV, credential.api_key, quote_mode="always")
        set_key(path, ORCA_KEY_SOURCE_ENV, "pkce", quote_mode="always")
        try:
            os.chmod(path, 0o600)
        except OSError:
            pass
        self._environ[ORCA_KEY_ENV] = credential.api_key  # type: ignore[index]
        self._environ[ORCA_KEY_SOURCE_ENV] = "pkce"  # type: ignore[index]

    def store(self, credential: OrcaRouterCredential) -> None:
        """Persist a PKCE credential through the shared credential interface."""
        self.save(credential)

    def clear(self) -> bool:
        from dotenv import unset_key

        path = self._file()
        existed = bool((self._environ.get(ORCA_KEY_ENV) or "").strip())
        if os.path.exists(path):
            unset_key(path, ORCA_KEY_ENV)
            unset_key(path, ORCA_KEY_SOURCE_ENV)
        self._environ.pop(ORCA_KEY_ENV, None)  # type: ignore[union-attr]
        self._environ.pop(ORCA_KEY_SOURCE_ENV, None)  # type: ignore[union-attr]
        return existed


def format_granted_scope_warning(credential: OrcaRouterCredential) -> str | None:
    """Say so when the grant is narrower or wider than what we asked for."""
    scope = credential.granted_scope
    if scope is None or scope == REQUESTED_SCOPE:
        return None
    return (
        f"OrcaRouter granted the scope {scope!r} rather than {REQUESTED_SCOPE!r}; "
        f"the key is still usable, but the workspace role did not permit the "
        f"requested grant."
    )


def encode_attempt_for_log(attempt: PkceAttempt) -> str:
    """Return a log-safe description of an attempt: never the verifier."""
    return json.dumps({"state_len": len(attempt.state), "challenge": attempt.code_challenge})


__all__ = [
    "APP_NAME",
    "CHALLENGE_METHOD",
    "CODE_TTL_SECONDS",
    "ExchangeResult",
    "LoopbackListener",
    "OrcaRouterAuthError",
    "OrcaRouterCredentialStore",
    "PkceAttempt",
    "build_authorize_url",
    "code_challenge_for",
    "connect",
    "connect_via_loopback",
    "connect_via_out_of_band",
    "encode_attempt_for_log",
    "format_granted_scope_warning",
    "generate_state",
    "generate_verifier",
    "parse_exchange_payload",
    "post_exchange",
]

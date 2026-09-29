"""PKCE protocol tests: verifier, challenge, state, authorize URL, exchange.

Every test here runs against the project's own connect adapter with a local
fake authorization server. No test performs a real authorization or holds a
real credential.
"""

import http.server
import json
import threading
import urllib.parse

import pytest

from cua_agent.orcarouter import (
    APP_NAME,
    OrcaRouterAuthError,
    OrcaRouterCredential,
    OrcaRouterCredentialStore,
    PkceAttempt,
    build_authorize_url,
    code_challenge_for,
    connect_via_loopback,
    connect_via_out_of_band,
    exchange_url,
    format_granted_scope_warning,
    parse_exchange_payload,
    post_exchange,
)
from cua_agent.orcarouter.credentials import same_secret
from tests.orcarouter.conftest import FAKE_PKCE_KEY


class TestVerifierAndChallenge:
    def test_challenge_is_unpadded_base64url_sha256(self):
        import base64
        import hashlib

        attempt = PkceAttempt.create()
        expected = (
            base64.urlsafe_b64encode(hashlib.sha256(attempt.verifier.encode()).digest())
            .decode()
            .rstrip("=")
        )
        assert attempt.code_challenge == expected
        assert "=" not in attempt.code_challenge
        assert "+" not in attempt.code_challenge and "/" not in attempt.code_challenge

    def test_every_attempt_uses_a_fresh_verifier_and_state(self):
        attempts = [PkceAttempt.create() for _ in range(25)]
        verifiers = {a.verifier for a in attempts}
        states = {a.state for a in attempts}
        assert len(verifiers) == 25
        assert len(states) == 25

    def test_verifier_is_high_entropy_and_not_derived_from_guessable_input(self):
        attempt = PkceAttempt.create()
        assert len(attempt.verifier) >= 43  # RFC 7636 minimum for S256
        assert attempt.verifier != code_challenge_for(attempt.verifier)

    def test_verifier_is_not_reused_across_attempts_of_the_same_helper(self):
        first = PkceAttempt.create()
        second = PkceAttempt.create()
        assert first.verifier != second.verifier
        assert first.code_challenge != second.code_challenge


class TestAuthorizeUrl:
    def test_uses_the_auth_origin_with_s256_and_oob(self):
        attempt = PkceAttempt.create()
        url = build_authorize_url(
            attempt, auth_base_url="https://www.orcarouter.ai", callback_url="oob"
        )
        parsed = urllib.parse.urlsplit(url)
        assert parsed.scheme == "https"
        assert parsed.netloc == "www.orcarouter.ai"
        assert parsed.path == "/auth"

        params = urllib.parse.parse_qs(parsed.query)
        assert params["callback_url"] == ["oob"]
        assert params["code_challenge_method"] == ["S256"]
        assert params["code_challenge"] == [attempt.code_challenge]
        assert params["state"] == [attempt.state]
        assert params["scope"] == ["api"]
        assert params["app_name"] == [APP_NAME]

    def test_never_carries_the_verifier(self):
        attempt = PkceAttempt.create()
        for callback in ("oob", "http://127.0.0.1:1234/cb"):
            url = build_authorize_url(
                attempt, auth_base_url="https://www.orcarouter.ai", callback_url=callback
            )
            assert attempt.verifier not in url

    def test_never_points_at_the_inference_origin(self):
        attempt = PkceAttempt.create()
        url = build_authorize_url(
            attempt, auth_base_url="https://www.orcarouter.ai", callback_url="oob"
        )
        assert "api.orcarouter.ai" not in url

    def test_loopback_callback_is_used_for_flow_a(self):
        attempt = PkceAttempt.create()
        url = build_authorize_url(
            attempt,
            auth_base_url="https://www.orcarouter.ai",
            callback_url="http://127.0.0.1:51733/cb",
        )
        params = urllib.parse.parse_qs(urllib.parse.urlsplit(url).query)
        assert params["callback_url"] == ["http://127.0.0.1:51733/cb"]


class TestExchangeResponse:
    def test_success_yields_a_durable_key_and_the_granted_scope(self):
        result = parse_exchange_payload(
            200, {"key": FAKE_PKCE_KEY, "user_id": "42", "scope": "api"}
        )
        credential = result.to_credential()
        assert credential.api_key == FAKE_PKCE_KEY
        assert credential.source == "pkce"
        assert credential.account_id == "42"
        assert credential.granted_scope == "api"

    def test_403_is_terminal(self):
        with pytest.raises(OrcaRouterAuthError) as caught:
            parse_exchange_payload(403, {"error": "invalid_grant"})
        assert caught.value.kind == "exchange_rejected"

    def test_400_reports_a_protocol_mismatch(self):
        with pytest.raises(OrcaRouterAuthError) as caught:
            parse_exchange_payload(400, {})
        assert caught.value.kind == "protocol"

    def test_429_reports_the_key_quota(self):
        with pytest.raises(OrcaRouterAuthError) as caught:
            parse_exchange_payload(429, {})
        assert caught.value.kind == "rate_limited"
        assert "10" in str(caught.value)

    def test_500_is_reported_as_an_upstream_failure(self):
        with pytest.raises(OrcaRouterAuthError) as caught:
            parse_exchange_payload(503, {})
        assert caught.value.kind == "network"

    def test_missing_key_is_rejected(self):
        with pytest.raises(OrcaRouterAuthError):
            parse_exchange_payload(200, {"user_id": "1", "scope": "api"})

    def test_narrower_grant_is_reported_not_hidden(self):
        result = parse_exchange_payload(200, {"key": FAKE_PKCE_KEY, "scope": "read"})
        credential = result.to_credential()
        warning = format_granted_scope_warning(credential)
        assert warning is not None
        assert "read" in warning

    def test_matching_grant_produces_no_warning(self):
        result = parse_exchange_payload(200, {"key": FAKE_PKCE_KEY, "scope": "api"})
        assert format_granted_scope_warning(result.to_credential()) is None

    def test_error_bodies_never_leak_into_the_raised_message(self):
        with pytest.raises(OrcaRouterAuthError) as caught:
            parse_exchange_payload(403, {"key": FAKE_PKCE_KEY})
        assert FAKE_PKCE_KEY not in str(caught.value)


class FakeAuthServer:
    """A local stand-in for the OrcaRouter authorization service."""

    def __init__(self, *, response_status=200, response_body=None, deny=False):
        self.requests = []
        self.response_status = response_status
        self.response_body = (
            response_body
            if response_body is not None
            else {
                "key": FAKE_PKCE_KEY,
                "user_id": "7",
                "scope": "api",
            }
        )
        self.deny = deny
        outer = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_POST(self):  # noqa: N802
                length = int(self.headers.get("Content-Length", 0))
                raw = self.rfile.read(length).decode()
                try:
                    outer.requests.append((self.path, json.loads(raw)))
                except ValueError:
                    outer.requests.append((self.path, raw))

                if outer.deny:
                    self.send_response(403)
                    self.send_header("Content-Type", "application/json")
                    self.end_headers()
                    self.wfile.write(b'{"error":"invalid_grant"}')
                    return
                body = json.dumps(outer.response_body).encode()
                self.send_response(outer.response_status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *_args):
                pass

        self._server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)
        self._thread.start()

    @property
    def base_url(self):
        return f"http://127.0.0.1:{self._server.server_address[1]}"

    def stop(self):
        self._server.shutdown()
        self._server.server_close()
        if self._thread.is_alive():
            self._thread.join(timeout=2)


class TestExchangeRequest:
    def test_sends_the_correct_path_body_and_method(self):
        server = FakeAuthServer()
        try:
            attempt = PkceAttempt.create()
            url = exchange_url(server.base_url)
            assert url.endswith("/api/v1/auth/keys")
            result = post_exchange(url, attempt, "fake-code")
        finally:
            server.stop()

        assert result.api_key == FAKE_PKCE_KEY
        path, body = server.requests[0]
        assert path == "/api/v1/auth/keys"
        assert body["code"] == "fake-code"
        assert body["code_verifier"] == attempt.verifier
        assert body["code_challenge_method"] == "S256"

    def test_verifier_never_appears_in_the_request_path(self):
        server = FakeAuthServer()
        try:
            attempt = PkceAttempt.create()
            post_exchange(exchange_url(server.base_url), attempt, "fake-code")
        finally:
            server.stop()
        path, _body = server.requests[0]
        assert attempt.verifier not in path

    def test_rejected_code_raises_and_does_not_leak_the_verifier(self):
        server = FakeAuthServer(deny=True)
        try:
            attempt = PkceAttempt.create()
            with pytest.raises(OrcaRouterAuthError) as caught:
                post_exchange(exchange_url(server.base_url), attempt, "reused-code")
        finally:
            server.stop()
        assert caught.value.kind == "exchange_rejected"
        assert attempt.verifier not in str(caught.value)

    def test_network_failure_is_reported_not_hung(self):
        attempt = PkceAttempt.create()
        with pytest.raises(OrcaRouterAuthError) as caught:
            post_exchange("http://127.0.0.1:9/api/v1/auth/keys", attempt, "code", timeout=1.0)
        assert caught.value.kind == "network"


class TestFlowBOutOfBand:
    def test_full_flow_exchanges_and_persists(self, tmp_path):
        server = FakeAuthServer()
        try:
            auth_base = server.base_url
            credentials = []
            environ = {}
            store = OrcaRouterCredentialStore(env_file=str(tmp_path / ".env"), environ=environ)

            credential = connect_via_out_of_band(
                auth_base_url=auth_base,
                read_code=lambda: "pasted-code",
                open_browser=lambda url: credentials.append(url),
            )
            store.save(credential)
        finally:
            server.stop()

        assert credential.api_key == FAKE_PKCE_KEY
        assert "callback_url=oob" in credentials[0]
        assert "/auth?" in credentials[0]
        assert store.resolve().api_key == FAKE_PKCE_KEY
        path, body = server.requests[0]
        assert path == "/api/v1/auth/keys"
        assert body["code"] == "pasted-code"

    def test_denial_is_terminal_and_stores_nothing(self, tmp_path):
        server = FakeAuthServer(deny=True)
        store = OrcaRouterCredentialStore(env_file=str(tmp_path / ".env"), environ={})
        try:
            with pytest.raises(OrcaRouterAuthError):
                connect_via_out_of_band(
                    auth_base_url=server.base_url,
                    read_code=lambda: "code",
                    open_browser=lambda url: None,
                )
        finally:
            server.stop()
        assert store.resolve() is None

    def test_empty_code_is_a_cancel_not_a_hang(self):
        with pytest.raises(OrcaRouterAuthError) as caught:
            connect_via_out_of_band(
                auth_base_url="http://127.0.0.1:9",
                read_code=lambda: "   ",
                open_browser=lambda url: None,
            )
        assert caught.value.kind == "cancelled"

    def test_a_missing_browser_does_not_abort_the_flow(self, tmp_path):
        server = FakeAuthServer()
        try:

            def failing_opener(url):
                raise OSError("no browser here")

            credential = connect_via_out_of_band(
                auth_base_url=server.base_url,
                read_code=lambda: "code",
                open_browser=failing_opener,
            )
        finally:
            server.stop()
        assert credential.api_key == FAKE_PKCE_KEY


class TestFlowALoopback:
    def test_state_mismatch_is_rejected_before_the_code_is_used(self):
        """A callback carrying someone else's state must fail closed."""
        import urllib.request

        from cua_agent.orcarouter.pkce import _extract_callback

        with pytest.raises(OrcaRouterAuthError) as caught:
            _extract_callback({"state": ["attacker-state"], "code": ["stolen"]}, "our-state")
        assert caught.value.kind == "state_mismatch"

    def test_missing_state_is_rejected(self):
        from cua_agent.orcarouter.pkce import _extract_callback

        with pytest.raises(OrcaRouterAuthError):
            _extract_callback({"code": ["c"]}, "our-state")

    def test_denied_callback_is_reported_as_denied(self):
        from cua_agent.orcarouter.pkce import _extract_callback

        with pytest.raises(OrcaRouterAuthError) as caught:
            _extract_callback({"state": ["s"], "error": ["access_denied"]}, "s")
        assert caught.value.kind == "denied"

    def test_state_comparison_is_constant_time_helper(self):
        assert same_secret("abc", "abc") is True
        assert same_secret("abc", "abd") is False
        assert same_secret(None, "abc") is False

    def test_loopback_listener_binds_an_ephemeral_loopback_port(self):
        from cua_agent.orcarouter import LoopbackListener

        with LoopbackListener() as listener:
            assert listener.callback_url.startswith("http://127.0.0.1:")
            assert listener.callback_url.endswith("/cb")
            assert listener.port > 0

    def test_loopback_flow_completes_against_a_local_server(self):
        """Drive the real Flow A listener: authorize -> callback -> exchange."""
        import urllib.request

        from cua_agent.orcarouter import LoopbackListener

        server = FakeAuthServer()
        captured = {}

        def fake_browser(url):
            """Simulate the browser: read the redirect target and call it back."""
            captured["url"] = url
            parsed = urllib.parse.urlsplit(url)
            params = urllib.parse.parse_qs(parsed.query)
            callback = params["callback_url"][0]
            state = params["state"][0]
            with urllib.request.urlopen(  # noqa: S310 - loopback test URL
                f"{callback}?code=fake-code&state={state}", timeout=5
            ) as response:
                captured["page"] = response.read().decode()

        try:
            credential = connect_via_loopback(
                auth_base_url=server.base_url,
                open_browser=fake_browser,
                timeout=10,
            )
        finally:
            server.stop()

        assert credential.api_key == FAKE_PKCE_KEY
        assert "Connected" in captured["page"]
        assert "/api/v1/auth/keys" == server.requests[0][0]

    def test_loopback_timeout_ends_cleanly(self):
        from cua_agent.orcarouter import LoopbackListener

        with LoopbackListener() as listener:
            listener.start("state")
            with pytest.raises(OrcaRouterAuthError) as caught:
                listener.wait(timeout=0.2)
            assert caught.value.kind == "expired"

    def test_loopback_flow_surfaces_a_denial_from_the_browser(self):
        import urllib.request

        server = FakeAuthServer()

        def denying_browser(url):
            params = urllib.parse.parse_qs(urllib.parse.urlsplit(url).query)
            callback = params["callback_url"][0]
            state = params["state"][0]
            with urllib.request.urlopen(  # noqa: S310 - loopback test URL
                f"{callback}?error=access_denied&state={state}", timeout=5
            ):
                pass

        try:
            with pytest.raises(OrcaRouterAuthError) as caught:
                connect_via_loopback(
                    auth_base_url=server.base_url,
                    open_browser=denying_browser,
                    timeout=10,
                )
        finally:
            server.stop()
        assert caught.value.kind == "denied"

    def test_loopback_flow_rejects_a_foreign_state(self):
        import urllib.request

        server = FakeAuthServer()

        def wrong_state_browser(url):
            params = urllib.parse.parse_qs(urllib.parse.urlsplit(url).query)
            callback = params["callback_url"][0]
            with urllib.request.urlopen(  # noqa: S310 - loopback test URL
                f"{callback}?code=fake-code&state=attacker", timeout=5
            ):
                pass

        try:
            with pytest.raises(OrcaRouterAuthError) as caught:
                connect_via_loopback(
                    auth_base_url=server.base_url,
                    open_browser=wrong_state_browser,
                    timeout=10,
                )
        finally:
            server.stop()
        assert caught.value.kind == "state_mismatch"
        # The code was never exchanged.
        assert server.requests == []


class TestConnectAdapter:
    def test_connect_uses_the_listener_by_default_and_persists(self, tmp_path):
        import urllib.request

        from cua_agent.orcarouter import connect

        server = FakeAuthServer()
        store = OrcaRouterCredentialStore(env_file=str(tmp_path / ".env"), environ={})

        def fake_browser(url):
            params = urllib.parse.parse_qs(urllib.parse.urlsplit(url).query)
            callback = params["callback_url"][0]
            with urllib.request.urlopen(  # noqa: S310 - loopback test URL
                f"{callback}?code=fake-code&state={params['state'][0]}", timeout=5
            ):
                pass

        try:
            credential = connect(
                auth_base_url=server.base_url,
                store=store,
                open_browser=fake_browser,
            )
        finally:
            server.stop()

        assert credential.source == "pkce"
        assert store.resolve().api_key == FAKE_PKCE_KEY

    def test_connect_uses_oob_when_a_code_reader_is_supplied(self, tmp_path):
        from cua_agent.orcarouter import connect

        server = FakeAuthServer()
        store = OrcaRouterCredentialStore(env_file=str(tmp_path / ".env"), environ={})
        urls = []
        try:
            credential = connect(
                auth_base_url=server.base_url,
                store=store,
                read_code=lambda: "pasted",
                on_url=urls.append,
                open_browser=lambda url: None,
            )
        finally:
            server.stop()
        assert credential.api_key == FAKE_PKCE_KEY
        assert "callback_url=oob" in urls[0]

    def test_scope_downgrade_is_refused_before_persisting(self, tmp_path):
        from cua_agent.orcarouter import connect

        server = FakeAuthServer(response_body={"key": FAKE_PKCE_KEY, "scope": "connector"})
        store = OrcaRouterCredentialStore(env_file=str(tmp_path / ".env"), environ={})
        try:
            with pytest.raises(OrcaRouterAuthError) as caught:
                connect(
                    auth_base_url=server.base_url,
                    store=store,
                    read_code=lambda: "code",
                    open_browser=lambda url: None,
                )
        finally:
            server.stop()
        assert caught.value.kind == "scope_downgrade"
        assert store.resolve() is None

    def test_failed_reconnect_keeps_the_previous_key(self, tmp_path):
        from cua_agent.orcarouter import connect

        store = OrcaRouterCredentialStore(env_file=str(tmp_path / ".env"), environ={})
        store.save(OrcaRouterCredential(api_key=FAKE_PKCE_KEY, source="pkce"))

        server = FakeAuthServer(deny=True)
        try:
            with pytest.raises(OrcaRouterAuthError):
                connect(
                    auth_base_url=server.base_url,
                    store=store,
                    read_code=lambda: "code",
                    open_browser=lambda url: None,
                )
        finally:
            server.stop()

        assert store.resolve().api_key == FAKE_PKCE_KEY

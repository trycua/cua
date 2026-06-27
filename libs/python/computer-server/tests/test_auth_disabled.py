"""Tests for the CUA_AUTH_DISABLED trusted-proxy bypass.

When ``CUA_AUTH_DISABLED=1`` is set, computer-server must:

1. Short-circuit ``AuthenticationManager.auth`` without making any outbound HTTP call.
2. Short-circuit ``_require_auth`` (used by PTY endpoints and /cmd) without reading
   the ``X-Container-Name`` / ``X-API-Key`` headers.
3. Skip the WebSocket ``authenticate`` handshake and proceed directly to the
   command loop.

The flag is meant to be set when something else (the cloud api proxy) has already
validated the workspace API key, so computer-server can trust whatever reaches it.
When the flag is unset, all existing behavior must be preserved.
"""

from __future__ import annotations

from unittest.mock import patch

import pytest

try:
    from fastapi.testclient import TestClient

    from computer_server.main import AuthenticationManager, app
except Exception as import_error:  # pragma: no cover - environment-dependent
    pytest.skip(
        f"computer_server.main unavailable in this environment: {import_error}",
        allow_module_level=True,
    )


@pytest.fixture
def clean_env(monkeypatch):
    """Remove all env vars that influence auth/availability for a clean baseline."""
    for var in (
        "CONTAINER_NAME",
        "CUA_AUTH_DISABLED",
        "UNAVAILABLE_WITHOUT_CONTAINER_NAME",
        "UNAVAILABLE_WITHOUT_CONTAINER_NAME_RESPONSE_STATUS_CODE",
    ):
        monkeypatch.delenv(var, raising=False)
    return monkeypatch


@pytest.fixture
def client():
    return TestClient(app)


class TestAuthenticationManagerBypass:
    """Unit tests for AuthenticationManager.auth() with the bypass flag."""

    @pytest.mark.asyncio
    async def test_auth_disabled_returns_true_without_http_call(self, clean_env):
        clean_env.setenv("CUA_AUTH_DISABLED", "1")
        # Pretend we're in cloud mode so the old code would hit the API.
        clean_env.setenv("CONTAINER_NAME", "vm-abc")

        mgr = AuthenticationManager()

        # If aiohttp.ClientSession is constructed, the test should fail — the
        # bypass path must not make any outbound HTTP request.
        with patch("computer_server.main.aiohttp.ClientSession") as session_cls:
            session_cls.side_effect = AssertionError(
                "aiohttp.ClientSession must not be called when CUA_AUTH_DISABLED=1"
            )
            result = await mgr.auth("vm-abc", "any-key")

        assert result is True

    @pytest.mark.asyncio
    async def test_auth_disabled_ignores_vm_identity_mismatch(self, clean_env):
        """The bypass even waives the server-side container_name check."""
        clean_env.setenv("CUA_AUTH_DISABLED", "1")
        clean_env.setenv("CONTAINER_NAME", "vm-expected")

        mgr = AuthenticationManager()
        # Deliberately mismatched container name — should still succeed.
        with patch("computer_server.main.aiohttp.ClientSession") as session_cls:
            session_cls.side_effect = AssertionError("no HTTP call expected")
            result = await mgr.auth("vm-WRONG", "any-key")

        assert result is True

    @pytest.mark.asyncio
    async def test_auth_flag_unset_still_performs_local_dev_passthrough(self, clean_env):
        """With the flag unset and no CONTAINER_NAME, existing local-dev behavior applies."""
        mgr = AuthenticationManager()
        result = await mgr.auth("anything", "anything")
        assert result is True

    @pytest.mark.asyncio
    async def test_auth_flag_unset_mismatched_container_still_fails(self, clean_env):
        """With the flag unset and CONTAINER_NAME set, mismatched names fail closed (no regression)."""
        clean_env.setenv("CONTAINER_NAME", "vm-expected")
        mgr = AuthenticationManager()
        result = await mgr.auth("vm-wrong", "any-key")
        assert result is False


class TestRequireAuthHTTPBypass:
    """Integration tests for _require_auth via the /cmd endpoint."""

    def test_cmd_with_flag_set_succeeds_without_headers(self, clean_env, client):
        clean_env.setenv("CUA_AUTH_DISABLED", "1")
        clean_env.setenv("CONTAINER_NAME", "vm-abc")

        resp = client.post("/cmd", json={"command": "version", "params": {}})
        assert resp.status_code == 200, resp.text
        assert "success" in resp.text

    def test_cmd_with_flag_unset_requires_headers(self, clean_env, client):
        # CONTAINER_NAME set but no flag: /cmd should 401 without headers.
        clean_env.setenv("CONTAINER_NAME", "vm-abc")

        resp = client.post("/cmd", json={"command": "version", "params": {}})
        assert resp.status_code == 401, resp.text

    def test_pty_get_with_flag_set_passes_auth_gate(self, clean_env, client):
        """With flag set, PTY endpoints (which use _require_auth) should not 401.

        The specific PID doesn't exist, so we expect 404 — the point is we got
        past the auth gate.
        """
        clean_env.setenv("CUA_AUTH_DISABLED", "1")
        clean_env.setenv("CONTAINER_NAME", "vm-abc")

        resp = client.get("/pty/999999")
        assert resp.status_code == 404, resp.text

    def test_pty_get_with_flag_unset_returns_401(self, clean_env, client):
        clean_env.setenv("CONTAINER_NAME", "vm-abc")

        resp = client.get("/pty/999999")
        assert resp.status_code == 401, resp.text


class TestWebSocketAuthBypass:
    """Integration tests for the WebSocket /ws handshake with the bypass flag."""

    def test_websocket_with_flag_set_skips_authenticate_handshake(self, clean_env, client):
        """With CUA_AUTH_DISABLED=1, the client can send commands directly."""
        clean_env.setenv("CUA_AUTH_DISABLED", "1")
        clean_env.setenv("CONTAINER_NAME", "vm-abc")

        with client.websocket_connect("/ws") as ws:
            # No authenticate message — send a regular command immediately.
            ws.send_json({"command": "version", "params": {}})
            data = ws.receive_json()
            assert data["success"] is True

    def test_websocket_with_flag_unset_still_requires_authenticate(self, clean_env, client):
        """With the flag unset and CONTAINER_NAME set, the server expects an authenticate message first."""
        clean_env.setenv("CONTAINER_NAME", "vm-abc")

        with client.websocket_connect("/ws") as ws:
            # Send a command without authenticating — should be rejected.
            ws.send_json({"command": "version", "params": {}})
            data = ws.receive_json()
            assert data["success"] is False

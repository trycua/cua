"""Integration tests for UNAVAILABLE_WITHOUT_CONTAINER_NAME behavior.

These tests verify two things:

1. **Backwards compat** — when neither ``CONTAINER_NAME`` nor
   ``UNAVAILABLE_WITHOUT_CONTAINER_NAME`` is set, the server continues to
   operate in local development mode (no auth required, requests succeed).

2. **New behavior** — when ``UNAVAILABLE_WITHOUT_CONTAINER_NAME`` is truthy
   and ``CONTAINER_NAME`` is unset, requests are rejected with the status
   code configured by
   ``UNAVAILABLE_WITHOUT_CONTAINER_NAME_RESPONSE_STATUS_CODE`` (default 503)
   rather than being allowed through.
"""

from __future__ import annotations

import pytest
from fastapi.testclient import TestClient

try:
    from computer_server.main import _unavailable_status_code, app
except Exception as import_error:  # pragma: no cover - environment-dependent
    pytest.skip(
        f"computer_server.main unavailable in this environment: {import_error}",
        allow_module_level=True,
    )


@pytest.fixture
def clean_env(monkeypatch):
    """Remove all env vars that influence auth availability for a clean baseline."""
    for var in (
        "CONTAINER_NAME",
        "UNAVAILABLE_WITHOUT_CONTAINER_NAME",
        "UNAVAILABLE_WITHOUT_CONTAINER_NAME_RESPONSE_STATUS_CODE",
    ):
        monkeypatch.delenv(var, raising=False)
    return monkeypatch


@pytest.fixture
def client():
    return TestClient(app)


class TestUnavailableStatusCode:
    """Unit tests for the `_unavailable_status_code` helper."""

    def test_returns_none_when_both_unset(self, clean_env):
        assert _unavailable_status_code() is None

    def test_returns_none_when_container_name_set(self, clean_env):
        clean_env.setenv("CONTAINER_NAME", "vm-abc")
        clean_env.setenv("UNAVAILABLE_WITHOUT_CONTAINER_NAME", "true")
        # CONTAINER_NAME being set overrides the unavailable flag.
        assert _unavailable_status_code() is None

    def test_returns_default_503_when_flag_truthy_and_container_missing(self, clean_env):
        clean_env.setenv("UNAVAILABLE_WITHOUT_CONTAINER_NAME", "true")
        assert _unavailable_status_code() == 503

    @pytest.mark.parametrize("value", ["1", "true", "True", "YES", "y", "on"])
    def test_accepts_various_truthy_values(self, clean_env, value):
        clean_env.setenv("UNAVAILABLE_WITHOUT_CONTAINER_NAME", value)
        assert _unavailable_status_code() == 503

    @pytest.mark.parametrize("value", ["0", "false", "no", "", "random"])
    def test_rejects_falsy_values(self, clean_env, value):
        clean_env.setenv("UNAVAILABLE_WITHOUT_CONTAINER_NAME", value)
        assert _unavailable_status_code() is None

    def test_custom_status_code(self, clean_env):
        clean_env.setenv("UNAVAILABLE_WITHOUT_CONTAINER_NAME", "1")
        clean_env.setenv("UNAVAILABLE_WITHOUT_CONTAINER_NAME_RESPONSE_STATUS_CODE", "418")
        assert _unavailable_status_code() == 418

    def test_invalid_status_code_falls_back_to_503(self, clean_env):
        clean_env.setenv("UNAVAILABLE_WITHOUT_CONTAINER_NAME", "1")
        clean_env.setenv("UNAVAILABLE_WITHOUT_CONTAINER_NAME_RESPONSE_STATUS_CODE", "not-a-number")
        assert _unavailable_status_code() == 503


class TestCmdEndpoint:
    """Integration tests for the POST /cmd endpoint."""

    def test_backwards_compat_local_dev_allows_requests(self, clean_env, client):
        # No CONTAINER_NAME, no availability flag — old "local dev" behavior.
        resp = client.post("/cmd", json={"command": "version", "params": {}})
        assert resp.status_code == 200, resp.text
        assert "success" in resp.text

    def test_unavailable_flag_rejects_with_default_503(self, clean_env, client):
        clean_env.setenv("UNAVAILABLE_WITHOUT_CONTAINER_NAME", "true")
        resp = client.post("/cmd", json={"command": "version", "params": {}})
        assert resp.status_code == 503
        assert "CONTAINER_NAME" in resp.json()["detail"]

    def test_unavailable_flag_with_custom_status_code(self, clean_env, client):
        clean_env.setenv("UNAVAILABLE_WITHOUT_CONTAINER_NAME", "1")
        clean_env.setenv("UNAVAILABLE_WITHOUT_CONTAINER_NAME_RESPONSE_STATUS_CODE", "418")
        resp = client.post("/cmd", json={"command": "version", "params": {}})
        assert resp.status_code == 418

    def test_container_name_set_bypasses_unavailable_flag(self, clean_env, client):
        # CONTAINER_NAME being set means auth is required — but the unavailable
        # flag should NOT apply. Without valid creds, this should 401, not 503.
        clean_env.setenv("CONTAINER_NAME", "vm-xyz")
        clean_env.setenv("UNAVAILABLE_WITHOUT_CONTAINER_NAME", "true")
        resp = client.post("/cmd", json={"command": "version", "params": {}})
        assert resp.status_code == 401


class TestPtyEndpointAuthGate:
    """Integration tests for PTY endpoints (via `_require_auth`)."""

    def test_backwards_compat_local_dev_allows_access(self, clean_env, client):
        # Use a non-existent PID — we just want to verify we get past the auth gate.
        # If auth passes, we get 404 (PTY not found); if not, we get 401/503.
        resp = client.get("/pty/999999")
        assert resp.status_code == 404, resp.text

    def test_unavailable_flag_rejects_with_default_503(self, clean_env, client):
        clean_env.setenv("UNAVAILABLE_WITHOUT_CONTAINER_NAME", "true")
        resp = client.get("/pty/999999")
        assert resp.status_code == 503

    def test_unavailable_flag_with_custom_status_code(self, clean_env, client):
        clean_env.setenv("UNAVAILABLE_WITHOUT_CONTAINER_NAME", "true")
        clean_env.setenv("UNAVAILABLE_WITHOUT_CONTAINER_NAME_RESPONSE_STATUS_CODE", "599")
        resp = client.get("/pty/999999")
        assert resp.status_code == 599


class TestPlaywrightExecEndpoint:
    def test_backwards_compat_local_dev_accepts_auth(self, clean_env, client):
        # Browser manager may fail for other reasons, but it should NOT be 503/401.
        resp = client.post("/playwright_exec", json={"command": "noop", "params": {}})
        assert resp.status_code not in (401, 503)

    def test_unavailable_flag_rejects(self, clean_env, client):
        clean_env.setenv("UNAVAILABLE_WITHOUT_CONTAINER_NAME", "true")
        resp = client.post("/playwright_exec", json={"command": "noop", "params": {}})
        assert resp.status_code == 503


class TestStatusEndpointMiddlewareGating:
    """The middleware applies uniformly — /status is reachable when the flag is off."""

    def test_status_accessible_in_local_dev_mode(self, clean_env, client):
        resp = client.get("/status")
        assert resp.status_code == 200
        assert resp.json()["status"] == "ok"

    def test_status_rejected_by_middleware_when_flag_set(self, clean_env, client):
        clean_env.setenv("UNAVAILABLE_WITHOUT_CONTAINER_NAME", "true")
        resp = client.get("/status")
        assert resp.status_code == 503

    def test_status_accessible_when_container_name_set(self, clean_env, client):
        clean_env.setenv("CONTAINER_NAME", "vm-abc")
        clean_env.setenv("UNAVAILABLE_WITHOUT_CONTAINER_NAME", "true")
        resp = client.get("/status")
        assert resp.status_code == 200


class TestWebSocketEndpoint:
    def test_backwards_compat_local_dev_allows_commands(self, clean_env, client):
        with client.websocket_connect("/ws") as ws:
            ws.send_json({"command": "version", "params": {}})
            data = ws.receive_json()
            assert data["success"] is True

    def test_unavailable_flag_closes_with_error(self, clean_env, client):
        clean_env.setenv("UNAVAILABLE_WITHOUT_CONTAINER_NAME", "true")
        with client.websocket_connect("/ws") as ws:
            data = ws.receive_json()
            assert data["success"] is False
            assert data["status_code"] == 503
            assert "CONTAINER_NAME" in data["error"]

    def test_unavailable_flag_reports_custom_status_code(self, clean_env, client):
        clean_env.setenv("UNAVAILABLE_WITHOUT_CONTAINER_NAME", "true")
        clean_env.setenv("UNAVAILABLE_WITHOUT_CONTAINER_NAME_RESPONSE_STATUS_CODE", "599")
        with client.websocket_connect("/ws") as ws:
            data = ws.receive_json()
            assert data["success"] is False
            assert data["status_code"] == 599

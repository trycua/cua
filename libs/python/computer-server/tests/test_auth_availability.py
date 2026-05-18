"""Integration tests for UNAVAILABLE_WITHOUT_CONTAINER_NAME behavior.

These tests verify two things:

1. **Safer default** — when neither ``CONTAINER_NAME`` nor
   ``CUA_ALLOW_UNAUTHENTICATED_LOCAL`` is set, control endpoints reject
   unauthenticated requests.

2. **Explicit local mode** — when ``CUA_ALLOW_UNAUTHENTICATED_LOCAL`` is truthy,
   local development requests continue to work.

3. **Unavailable behavior** — when ``UNAVAILABLE_WITHOUT_CONTAINER_NAME`` is truthy
   and ``CONTAINER_NAME`` is unset, requests are rejected with the status
   code configured by
   ``UNAVAILABLE_WITHOUT_CONTAINER_NAME_RESPONSE_STATUS_CODE`` (default 503)
   rather than being allowed through.
"""

from __future__ import annotations

import pytest
from fastapi.testclient import TestClient

try:
    import computer_server.main as main
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
        "CUA_ALLOW_UNAUTHENTICATED_LOCAL",
        "CUA_ENABLE_RUN_COMMAND",
        "CUA_ENABLE_PUBLIC_PROXY",
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

    def test_default_local_dev_rejects_requests(self, clean_env, client):
        resp = client.post("/cmd", json={"command": "version", "params": {}})
        assert resp.status_code == 401, resp.text
        assert "Authentication required" in resp.json()["detail"]

    def test_explicit_local_dev_allows_requests(self, clean_env, client):
        clean_env.setenv("CUA_ALLOW_UNAUTHENTICATED_LOCAL", "1")
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

    def test_run_command_disabled_by_default(self, clean_env, client):
        clean_env.setenv("CUA_ALLOW_UNAUTHENTICATED_LOCAL", "1")
        resp = client.post("/cmd", json={"command": "shell", "params": {"command": "echo hi"}})
        assert resp.status_code == 403
        assert "CUA_ENABLE_RUN_COMMAND" in resp.json()["detail"]


class TestPtyEndpointAuthGate:
    """Integration tests for PTY endpoints (via `_require_auth`)."""

    def test_default_local_dev_rejects_access(self, clean_env, client):
        resp = client.get("/pty/999999")
        assert resp.status_code == 401

    def test_explicit_local_dev_allows_access(self, clean_env, client):
        clean_env.setenv("CUA_ALLOW_UNAUTHENTICATED_LOCAL", "1")
        # Use a non-existent PID — we just want to verify we get past the auth gate.
        # If auth passes, we get 404 (PTY not found); if not, we get 401/503.
        resp = client.get("/pty/999999")
        assert resp.status_code == 404, resp.text

    def test_pty_creation_disabled_by_default(self, clean_env, client):
        clean_env.setenv("CUA_ALLOW_UNAUTHENTICATED_LOCAL", "1")
        resp = client.post("/pty", json={"command": "echo hi"})
        assert resp.status_code == 403
        assert "CUA_ENABLE_RUN_COMMAND" in resp.json()["detail"]

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
    def test_default_local_dev_rejects_auth(self, clean_env, client):
        resp = client.post("/playwright_exec", json={"command": "noop", "params": {}})
        assert resp.status_code == 401

    def test_explicit_local_dev_accepts_auth(self, clean_env, client):
        clean_env.setenv("CUA_ALLOW_UNAUTHENTICATED_LOCAL", "1")
        # Browser manager may fail for other reasons, but it should NOT be 503/401.
        resp = client.post("/playwright_exec", json={"command": "noop", "params": {}})
        assert resp.status_code not in (401, 503)

    def test_unavailable_flag_rejects(self, clean_env, client):
        clean_env.setenv("UNAVAILABLE_WITHOUT_CONTAINER_NAME", "true")
        resp = client.post("/playwright_exec", json={"command": "noop", "params": {}})
        assert resp.status_code == 503


class TestAgentResponsesEndpoint:
    def test_default_local_dev_rejects_auth_before_body_validation(self, clean_env, client):
        resp = client.post("/responses", json={})
        if not main.HAS_AGENT:
            assert resp.status_code == 501
            return
        assert resp.status_code == 401
        assert "Authentication required" in resp.json()["detail"]

    def test_explicit_local_dev_reaches_body_validation(self, clean_env, client):
        clean_env.setenv("CUA_ALLOW_UNAUTHENTICATED_LOCAL", "1")
        resp = client.post("/responses", json={})
        if not main.HAS_AGENT:
            assert resp.status_code == 501
            return
        assert resp.status_code == 400
        assert "'model' and 'input' are required" in resp.json()["detail"]


class TestCommandsEndpoint:
    def test_static_command_metadata_available_before_handler_initialization(self, clean_env, client):
        resp = client.get("/commands")
        assert resp.status_code == 200

        commands = resp.json()["commands"]
        assert commands["launch"]["params"] == [
            {"name": "app", "required": True, "default": None},
            {"name": "args", "required": False, "default": None},
        ]
        assert commands["run_command"]["params"] == [
            {"name": "command", "required": True, "default": None},
            {"name": "timeout", "required": False, "default": None},
        ]
        assert commands["write_bytes"]["params"] == [
            {"name": "path", "required": True, "default": None},
            {"name": "content_b64", "required": True, "default": None},
            {"name": "append", "required": False, "default": False},
        ]

    def test_fresh_android_commands_metadata_lists_multitouch(
        self, clean_env, client, monkeypatch
    ):
        monkeypatch.setattr(main, "OS_TYPE", "android")
        monkeypatch.setattr(main, "_command_handlers", None)

        resp = client.get("/commands")
        assert resp.status_code == 200
        assert resp.json()["commands"]["multitouch_gesture"]["params"] == [
            {"name": "fingers", "required": True, "default": None},
            {"name": "screen_w", "required": True, "default": None},
            {"name": "screen_h", "required": True, "default": None},
            {"name": "duration_ms", "required": False, "default": 400},
            {"name": "steps", "required": False, "default": 0},
        ]

    def test_dynamic_android_command_initializes_before_unknown_rejection(
        self, clean_env, client, monkeypatch
    ):
        async def fake_multitouch_gesture():
            return {"success": True}

        monkeypatch.setattr(main, "OS_TYPE", "android")
        monkeypatch.setattr(main, "_command_handlers", None)
        monkeypatch.setattr(
            main,
            "_get_command_handlers",
            lambda: {"multitouch_gesture": fake_multitouch_gesture},
        )
        clean_env.setenv("CUA_ALLOW_UNAUTHENTICATED_LOCAL", "1")

        resp = client.post("/cmd", json={"command": "multitouch_gesture", "params": {}})
        assert resp.status_code == 200
        assert '"success": true' in resp.text

    def test_non_android_multitouch_rejection_does_not_initialize_handlers(
        self, clean_env, client, monkeypatch
    ):
        def fail_get_handlers():
            raise AssertionError("should not initialize handlers")

        monkeypatch.setattr(main, "OS_TYPE", "darwin")
        monkeypatch.setattr(main, "_get_command_handlers", fail_get_handlers)
        clean_env.setenv("CUA_ALLOW_UNAUTHENTICATED_LOCAL", "1")

        resp = client.post("/cmd", json={"command": "multitouch_gesture", "params": {}})
        assert resp.status_code == 400
        assert "Unknown command" in resp.json()["detail"]


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
    def test_default_local_dev_rejects_commands(self, clean_env, client):
        with client.websocket_connect("/ws") as ws:
            data = ws.receive_json()
            assert data["success"] is False
            assert data["status_code"] == 401
            assert "Authentication required" in data["error"]

    def test_explicit_local_dev_allows_commands(self, clean_env, client):
        clean_env.setenv("CUA_ALLOW_UNAUTHENTICATED_LOCAL", "1")
        with client.websocket_connect("/ws") as ws:
            ws.send_json({"command": "version", "params": {}})
            data = ws.receive_json()
            assert data["success"] is True

    def test_run_command_disabled_by_default(self, clean_env, client):
        clean_env.setenv("CUA_ALLOW_UNAUTHENTICATED_LOCAL", "1")
        with client.websocket_connect("/ws") as ws:
            ws.send_json({"command": "shell", "params": {"command": "echo hi"}})
            data = ws.receive_json()
            assert data["success"] is False
            assert "CUA_ENABLE_RUN_COMMAND" in data["error"]

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

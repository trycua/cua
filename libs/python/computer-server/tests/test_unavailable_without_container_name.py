"""Integration tests for the UNAVAILABLE_WITHOUT_CONTAINER_NAME auth gate.

Covers both scenarios:
  1. Gate disabled (default): requests fall through to local development mode.
  2. Gate enabled + CONTAINER_NAME unset: requests are rejected with the
     configured HTTP status code (default 503) for /cmd and /responses, and
     the WebSocket handshake is closed with an error payload containing the
     same status code.

The real HandlerFactory pulls in OS-specific automation libraries (pynput on
Linux) that require an X display, so we stub it in sys.modules before
importing computer_server.main.
"""

from __future__ import annotations

import importlib
import sys
import types
from unittest.mock import MagicMock

import pytest


def _install_handler_factory_stub() -> None:
    """Register a no-op HandlerFactory so main.py can import without a display."""
    if "computer_server.handlers.factory" in sys.modules:
        existing = sys.modules["computer_server.handlers.factory"]
        if getattr(existing, "__cua_test_stub__", False):
            return

    stub_module = types.ModuleType("computer_server.handlers.factory")
    stub_module.__cua_test_stub__ = True  # type: ignore[attr-defined]

    class _StubHandlerFactory:
        @staticmethod
        def create_handlers():
            return (MagicMock(), MagicMock(), MagicMock(), MagicMock())

    stub_module.HandlerFactory = _StubHandlerFactory  # type: ignore[attr-defined]
    sys.modules["computer_server.handlers.factory"] = stub_module


def _reload_main(monkeypatch, **env: str):
    """Reload computer_server.main with the given env vars applied.

    Module-level constants (UNAVAILABLE_WITHOUT_CONTAINER_NAME and its status
    code companion) are captured at import time, so we must reload after
    mutating the environment.
    """
    _install_handler_factory_stub()

    for var in (
        "CONTAINER_NAME",
        "UNAVAILABLE_WITHOUT_CONTAINER_NAME",
        "UNAVAILABLE_WITHOUT_CONTAINER_NAME_RESPONSE_STATUS_CODE",
    ):
        monkeypatch.delenv(var, raising=False)
    for key, value in env.items():
        monkeypatch.setenv(key, value)

    if "computer_server.main" in sys.modules:
        return importlib.reload(sys.modules["computer_server.main"])
    return importlib.import_module("computer_server.main")


@pytest.fixture
def test_client_factory(monkeypatch):
    """Return a callable that builds a TestClient with the requested env."""
    try:
        from fastapi.testclient import TestClient
    except ImportError:
        pytest.skip("fastapi.testclient (httpx) not installed")

    def _build(**env: str):
        main = _reload_main(monkeypatch, **env)
        return main, TestClient(main.app)

    return _build


class TestGateDisabledPassthrough:
    """Scenario 1: UNAVAILABLE_WITHOUT_CONTAINER_NAME unset -> local dev mode."""

    def test_constants_default_to_disabled_and_503(self, test_client_factory):
        main, _ = test_client_factory()
        assert main.UNAVAILABLE_WITHOUT_CONTAINER_NAME is False
        assert main.UNAVAILABLE_WITHOUT_CONTAINER_NAME_RESPONSE_STATUS_CODE == 503

    def test_cmd_passes_gate_and_reaches_handler_lookup(self, test_client_factory):
        # With the gate disabled and CONTAINER_NAME unset, the gate short-circuit
        # is skipped and the request flows into the handler dispatch. An unknown
        # command produces a 400 -- proving the 503 gate did not fire.
        _, client = test_client_factory()
        resp = client.post("/cmd", json={"command": "__definitely_not_a_real_command__"})
        assert resp.status_code == 400
        assert "Unknown command" in resp.json()["detail"]

    def test_ws_accepts_connection_without_auth_handshake(self, test_client_factory):
        _, client = test_client_factory()
        with client.websocket_connect("/ws") as ws:
            ws.send_json({"command": "__definitely_not_a_real_command__"})
            msg = ws.receive_json()
        assert msg["success"] is False
        assert "Unknown command" in msg["error"]


class TestGateEnabledRejection:
    """Scenario 2: UNAVAILABLE_WITHOUT_CONTAINER_NAME=true + no CONTAINER_NAME."""

    def test_constants_reflect_enabled_gate(self, test_client_factory):
        main, _ = test_client_factory(UNAVAILABLE_WITHOUT_CONTAINER_NAME="true")
        assert main.UNAVAILABLE_WITHOUT_CONTAINER_NAME is True
        assert main.UNAVAILABLE_WITHOUT_CONTAINER_NAME_RESPONSE_STATUS_CODE == 503

    def test_cmd_returns_default_503(self, test_client_factory):
        _, client = test_client_factory(UNAVAILABLE_WITHOUT_CONTAINER_NAME="true")
        resp = client.post("/cmd", json={"command": "version"})
        assert resp.status_code == 503
        assert "CONTAINER_NAME not set" in resp.json()["detail"]

    @pytest.mark.parametrize("truthy", ["1", "true", "yes", "y", "on", "TRUE", "On"])
    def test_cmd_gate_accepts_truthy_spellings(self, test_client_factory, truthy):
        _, client = test_client_factory(UNAVAILABLE_WITHOUT_CONTAINER_NAME=truthy)
        resp = client.post("/cmd", json={"command": "version"})
        assert resp.status_code == 503

    def test_cmd_honors_custom_status_code(self, test_client_factory):
        _, client = test_client_factory(
            UNAVAILABLE_WITHOUT_CONTAINER_NAME="1",
            UNAVAILABLE_WITHOUT_CONTAINER_NAME_RESPONSE_STATUS_CODE="418",
        )
        resp = client.post("/cmd", json={"command": "version"})
        assert resp.status_code == 418
        assert "CONTAINER_NAME not set" in resp.json()["detail"]

    def test_ws_sends_rejection_payload_and_closes(self, test_client_factory):
        from starlette.websockets import WebSocketDisconnect

        _, client = test_client_factory(
            UNAVAILABLE_WITHOUT_CONTAINER_NAME="true",
            UNAVAILABLE_WITHOUT_CONTAINER_NAME_RESPONSE_STATUS_CODE="521",
        )
        with client.websocket_connect("/ws") as ws:
            msg = ws.receive_json()
            assert msg["success"] is False
            assert msg["status_code"] == 521
            assert "CONTAINER_NAME not set" in msg["error"]
            with pytest.raises(WebSocketDisconnect):
                ws.receive_json()

    def test_gate_ignored_when_container_name_is_set(self, test_client_factory):
        # When CONTAINER_NAME is set, the gate is irrelevant: the cloud auth
        # path runs instead, rejecting missing headers with 401 (not 503).
        _, client = test_client_factory(
            CONTAINER_NAME="my-container",
            UNAVAILABLE_WITHOUT_CONTAINER_NAME="true",
        )
        resp = client.post("/cmd", json={"command": "version"})
        assert resp.status_code == 401

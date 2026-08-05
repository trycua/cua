"""Tests for the PTY HTTP and WebSocket authentication contract."""

import asyncio
import importlib.util
import sys
from pathlib import Path

import aiohttp
import pytest


class _WebSocketContext:
    def __init__(self, websocket):
        self.websocket = websocket

    async def __aenter__(self):
        return self.websocket

    async def __aexit__(self, exc_type, exc, tb):
        return False


class _EmptyWebSocket:
    def __aiter__(self):
        return self

    async def __anext__(self):
        raise StopAsyncIteration

    async def send_str(self, payload):
        raise AssertionError(f"no stdin should be sent in this test: {payload}")


class _Session:
    def __init__(self, capture):
        self.capture = capture

    async def __aenter__(self):
        return self

    async def __aexit__(self, exc_type, exc, tb):
        return False

    def ws_connect(self, url, **kwargs):
        self.capture["url"] = url
        self.capture["kwargs"] = kwargs
        return _WebSocketContext(_EmptyWebSocket())


@pytest.mark.asyncio
async def test_pty_websocket_auth_uses_headers_not_query_parameters(monkeypatch):
    module_path = Path(__file__).parents[1] / "computer" / "pty.py"
    spec = importlib.util.spec_from_file_location("_computer_pty_auth_test", module_path)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)

    capture = {}
    monkeypatch.setattr(aiohttp, "ClientSession", lambda: _Session(capture))

    interface = module.PtyInterface(
        "https://computer.example",
        api_key="secret-api-key",
        vm_name="container-123",
    )
    exit_event = asyncio.Event()

    await interface._ws_reader(42, None, exit_event, [0])

    assert capture["url"] == "wss://computer.example/pty/42/ws"
    assert capture["kwargs"] == {
        "headers": {
            "X-API-Key": "secret-api-key",
            "X-Container-Name": "container-123",
        }
    }
    assert "secret-api-key" not in capture["url"]
    assert "container-123" not in capture["url"]
    assert exit_event.is_set()

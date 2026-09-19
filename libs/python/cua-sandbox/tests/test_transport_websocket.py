"""Unit tests for the computer-server WebSocket command protocol."""

from __future__ import annotations

import base64
import json
from io import BytesIO
from unittest.mock import AsyncMock

import pytest
from cua_sandbox.transport.websocket import WebSocketTransport
from PIL import Image

pytestmark = pytest.mark.asyncio

PNG = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC"
)
PNG_BASE64 = base64.b64encode(PNG).decode()


def _make_transport(response) -> WebSocketTransport:
    transport = WebSocketTransport("ws://server:8000/ws")
    transport._ws = AsyncMock()
    transport._ws.recv.return_value = json.dumps(response)
    return transport


@pytest.mark.parametrize(
    "action, params, response, expected",
    [
        (
            "read_text",
            {"path": "/tmp/example.txt"},
            {"success": True, "content": "hello"},
            {"success": True, "content": "hello"},
        ),
        (
            "read_text",
            {"path": "/tmp/example.txt"},
            {"result": {"content": "hello"}},
            {"content": "hello"},
        ),
        (
            "get_cursor_position",
            {},
            {"success": True, "x": 10, "y": 20},
            {"success": True, "x": 10, "y": 20},
        ),
        (
            "run_command",
            {"command": "echo hello"},
            {"success": True, "stdout": "hello\n"},
            {"success": True, "stdout": "hello\n"},
        ),
    ],
    ids=["server-response", "legacy-result", "parameterless", "command-parameter"],
)
async def test_send_command(action, params, response, expected):
    transport = _make_transport(response)

    result = await transport.send(action, **params)

    transport._ws.send.assert_awaited_once()
    payload = json.loads(transport._ws.send.call_args.args[0])
    assert payload["command"] == action
    assert payload.get("params", {}) == params
    assert set(payload) <= {"command", "params"}
    assert result == expected


@pytest.mark.parametrize(
    "response",
    [
        {"success": True, "image_data": PNG_BASE64, "format": "png"},
        {"success": True, "screenshot": PNG_BASE64},
        {"result": PNG_BASE64},
        {"result": {"base64": PNG_BASE64}},
    ],
    ids=["server-image-data", "legacy-screenshot", "legacy-result", "legacy-base64"],
)
async def test_screenshot_response_formats(response):
    transport = _make_transport(response)

    assert await transport.screenshot() == PNG


@pytest.mark.parametrize("format", ["png", "jpeg", "jpg"])
async def test_screenshot_conversion(format):
    transport = _make_transport({"result": PNG_BASE64})

    data = await transport.screenshot(format=format, quality=73)

    with Image.open(BytesIO(data)) as image:
        assert image.size == (1, 1)
        assert image.format == ("PNG" if format == "png" else "JPEG")
        if format != "png":
            assert image.mode == "RGB"
    if format == "png":
        assert data == PNG


@pytest.mark.parametrize(
    "response",
    [
        {"success": True, "size": {"width": 1920, "height": 1080}},
        {"width": 1920, "height": 1080},
        {"result": {"width": 1920, "height": 1080}},
    ],
    ids=["server-size", "legacy-direct", "legacy-result"],
)
async def test_get_screen_size_response_formats(response):
    transport = _make_transport(response)

    assert await transport.get_screen_size() == {"width": 1920, "height": 1080}


@pytest.mark.parametrize("method", ["send", "screenshot", "get_screen_size", "get_environment"])
@pytest.mark.parametrize(
    "response, message",
    [
        ({"success": False, "error": "permission denied"}, "permission denied"),
        ({"success": False, "error": ""}, "Command failed"),
        ({"success": False}, "Command failed"),
    ],
)
async def test_remote_errors(method, response, message):
    transport = _make_transport(response)
    args = ("read_text",) if method == "send" else ()

    with pytest.raises(RuntimeError, match=f"Remote error: {message}"):
        await getattr(transport, method)(*args)

"""Integration tests specifically for the HTTP transport against a running computer-server.

Skipped unless CUA_TEST_HTTP_URL is set.
"""

from __future__ import annotations

import pytest

pytestmark = pytest.mark.asyncio


class TestHTTPTransport:
    async def test_screenshot(self, http_transport):
        data = await http_transport.screenshot()
        assert isinstance(data, bytes)
        assert data[:4] == b"\x89PNG"

    async def test_screen_size(self, http_transport):
        size = await http_transport.get_screen_size()
        assert size["width"] > 0
        assert size["height"] > 0

    async def test_get_environment(self, http_transport):
        env = await http_transport.get_environment()
        assert env in ("windows", "mac", "linux", "browser")

    async def test_send_click(self, http_transport):
        result = await http_transport.send("left_click", x=100, y=100)
        # Should not raise

    async def test_send_type(self, http_transport):
        result = await http_transport.send("type_text", text="hello")

    async def test_send_get_screen_size(self, http_transport):
        result = await http_transport.send("get_screen_size")
        assert "width" in result or isinstance(result, dict)


class TestHTTPSandboxFullInterface:
    """Same interface tests as the main suite, but explicitly via HTTP sandbox."""

    async def test_screenshot(self, http_sandbox):
        data = await http_sandbox.screenshot()
        assert data[:4] == b"\x89PNG"

    async def test_mouse_click(self, http_sandbox):
        await http_sandbox.mouse.click(100, 100)

    async def test_keyboard_type(self, http_sandbox):
        await http_sandbox.keyboard.type("http-test")

    async def test_clipboard(self, http_sandbox):
        await http_sandbox.clipboard.set("http-clip")
        val = await http_sandbox.clipboard.get()
        assert val == "http-clip"

    async def test_shell(self, http_sandbox):
        result = await http_sandbox.shell.run("echo http-shell")
        assert result.success

    async def test_window_title(self, http_sandbox):
        title = await http_sandbox.window.get_active_title()
        assert isinstance(title, str)

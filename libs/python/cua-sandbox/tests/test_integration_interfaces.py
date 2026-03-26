"""Integration tests — exercise every interface method against every available transport.

Run with:
    pytest tests/test_integration_interfaces.py -v

By default only local transport is tested. Set env vars to enable remote:
    CUA_TEST_WS_URL=ws://host:8000/ws
    CUA_TEST_HTTP_URL=http://host:8000
"""

from __future__ import annotations

import asyncio

import pytest

pytestmark = pytest.mark.asyncio


# ═══════════════════════════════════════════════════════════════════════════════
# Screen
# ═══════════════════════════════════════════════════════════════════════════════

class TestScreen:
    async def test_screenshot_returns_png_bytes(self, any_sandbox):
        data = await any_sandbox.screenshot()
        assert isinstance(data, bytes)
        assert len(data) > 100
        # PNG magic bytes
        assert data[:4] == b"\x89PNG"

    async def test_screenshot_base64(self, any_sandbox):
        b64 = await any_sandbox.screenshot_base64()
        assert isinstance(b64, str)
        assert len(b64) > 100

    async def test_screen_size(self, any_sandbox):
        w, h = await any_sandbox.get_dimensions()
        assert isinstance(w, int)
        assert isinstance(h, int)
        assert w > 0
        assert h > 0


# ═══════════════════════════════════════════════════════════════════════════════
# Mouse
# ═══════════════════════════════════════════════════════════════════════════════

class TestMouse:
    async def test_move(self, any_sandbox):
        await any_sandbox.mouse.move(100, 100)

    async def test_click(self, any_sandbox):
        await any_sandbox.mouse.click(100, 100)

    async def test_right_click(self, any_sandbox):
        await any_sandbox.mouse.right_click(200, 200)

    async def test_double_click(self, any_sandbox):
        await any_sandbox.mouse.double_click(150, 150)

    async def test_scroll(self, any_sandbox):
        await any_sandbox.mouse.scroll(100, 100, scroll_y=3)

    async def test_mouse_down_up(self, any_sandbox):
        await any_sandbox.mouse.mouse_down(100, 100)
        await any_sandbox.mouse.mouse_up(200, 200)

    async def test_drag(self, any_sandbox):
        await any_sandbox.mouse.drag(100, 100, 300, 300)


# ═══════════════════════════════════════════════════════════════════════════════
# Keyboard
# ═══════════════════════════════════════════════════════════════════════════════

class TestKeyboard:
    async def test_type_text(self, any_sandbox):
        await any_sandbox.keyboard.type("hello")

    async def test_keypress_single(self, any_sandbox):
        await any_sandbox.keyboard.keypress("enter")

    async def test_keypress_combo(self, any_sandbox):
        await any_sandbox.keyboard.keypress(["ctrl", "a"])

    async def test_key_down_up(self, any_sandbox):
        await any_sandbox.keyboard.key_down("shift")
        await any_sandbox.keyboard.key_up("shift")


# ═══════════════════════════════════════════════════════════════════════════════
# Clipboard
# ═══════════════════════════════════════════════════════════════════════════════

class TestClipboard:
    async def test_set_and_get(self, any_sandbox):
        await any_sandbox.clipboard.set("cua-sandbox-test")
        result = await any_sandbox.clipboard.get()
        assert result == "cua-sandbox-test"


# ═══════════════════════════════════════════════════════════════════════════════
# Shell
# ═══════════════════════════════════════════════════════════════════════════════

class TestShell:
    async def test_run_echo(self, any_sandbox):
        result = await any_sandbox.shell.run("echo hello-sandbox")
        assert result.success
        assert "hello-sandbox" in result.stdout

    async def test_run_failure(self, any_sandbox):
        result = await any_sandbox.shell.run("exit 1")
        assert not result.success
        assert result.returncode != 0


# ═══════════════════════════════════════════════════════════════════════════════
# Window
# ═══════════════════════════════════════════════════════════════════════════════

class TestWindow:
    async def test_get_active_title(self, any_sandbox):
        title = await any_sandbox.window.get_active_title()
        assert isinstance(title, str)


# ═══════════════════════════════════════════════════════════════════════════════
# Environment / Dimensions
# ═══════════════════════════════════════════════════════════════════════════════

class TestEnvironment:
    async def test_get_environment(self, any_sandbox):
        env = await any_sandbox.get_environment()
        assert env in ("windows", "mac", "linux", "browser")


# ═══════════════════════════════════════════════════════════════════════════════
# Localhost (separate from sandbox parametrization)
# ═══════════════════════════════════════════════════════════════════════════════

class TestLocalhost:
    async def test_screenshot(self, localhost_instance):
        data = await localhost_instance.screenshot()
        assert data[:4] == b"\x89PNG"

    async def test_mouse_click(self, localhost_instance):
        await localhost_instance.mouse.click(50, 50)

    async def test_keyboard_type(self, localhost_instance):
        await localhost_instance.keyboard.type("x")

    async def test_shell_run(self, localhost_instance):
        result = await localhost_instance.shell.run("echo localhost-test")
        assert result.success

    async def test_environment(self, localhost_instance):
        env = await localhost_instance.get_environment()
        assert env in ("windows", "mac", "linux")

    async def test_dimensions(self, localhost_instance):
        w, h = await localhost_instance.get_dimensions()
        assert w > 0 and h > 0

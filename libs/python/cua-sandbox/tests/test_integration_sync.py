"""Integration tests for the synchronous API wrappers."""

from __future__ import annotations

import pytest

from cua_sandbox.sync import sandbox, localhost


class TestSyncSandbox:
    def test_screenshot(self):
        with sandbox(local=True) as sb:
            data = sb.screenshot()
            assert isinstance(data, bytes)
            assert data[:4] == b"\x89PNG"

    def test_mouse_click(self):
        with sandbox(local=True) as sb:
            sb.mouse.click(50, 50)

    def test_shell_run(self):
        with sandbox(local=True) as sb:
            result = sb.shell.run("echo sync-test")
            assert result.success
            assert "sync-test" in result.stdout

    def test_keyboard_type(self):
        with sandbox(local=True) as sb:
            sb.keyboard.type("x")

    def test_dimensions(self):
        with sandbox(local=True) as sb:
            w, h = sb.get_dimensions()
            assert w > 0 and h > 0


class TestSyncLocalhost:
    def test_screenshot(self):
        with localhost() as host:
            data = host.screenshot()
            assert isinstance(data, bytes)

    def test_shell_run(self):
        with localhost() as host:
            result = host.shell.run("echo sync-localhost")
            assert result.success

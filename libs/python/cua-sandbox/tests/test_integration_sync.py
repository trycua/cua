"""Integration tests for the synchronous API wrappers."""

from __future__ import annotations

from cua_sandbox.sync import localhost


class TestSyncLocalhost:
    def test_screenshot(self):
        with localhost() as host:
            data = host.screenshot()
            assert isinstance(data, bytes)
            assert data[:4] == b"\x89PNG"

    def test_mouse_click(self):
        with localhost() as host:
            host.mouse.click(50, 50)

    def test_shell_run(self):
        with localhost() as host:
            result = host.shell.run("echo sync-test")
            assert result.success
            assert "sync-test" in result.stdout

    def test_keyboard_type(self):
        with localhost() as host:
            host.keyboard.type("x")

    def test_dimensions(self):
        with localhost() as host:
            w, h = host.get_dimensions()
            assert w > 0 and h > 0

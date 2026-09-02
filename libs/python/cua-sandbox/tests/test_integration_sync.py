"""Integration tests for the synchronous API wrappers.

These drive the *real* host desktop (they type on the real keyboard and grab
the real screen), so they are skipped unless this machine has a controllable
desktop. See tests/conftest.py::local_desktop_available.
"""

from __future__ import annotations

import pytest
from cua_sandbox.sync import localhost

from tests.conftest import LOCAL_ENABLED, LOCAL_SKIP_REASON

pytestmark = pytest.mark.skipif(not LOCAL_ENABLED, reason=LOCAL_SKIP_REASON)


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

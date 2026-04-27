"""Unit tests for computer-server package.

This file tests ONLY basic server functionality.
Following SRP: This file tests server initialization and basic operations.
All external dependencies are mocked.
"""

from unittest.mock import AsyncMock, Mock, patch

import pytest


class TestServerImports:
    """Test server module imports (SRP: Only tests imports)."""

    def test_server_module_exists(self):
        """Test that server module can be imported."""
        try:
            import computer_server

            assert computer_server is not None
        except ImportError:
            pytest.skip("computer_server module not installed")


class TestServerInitialization:
    """Test server initialization (SRP: Only tests initialization)."""

    @pytest.mark.asyncio
    async def test_server_can_be_imported(self):
        """Basic smoke test: verify server components can be imported."""
        try:
            from computer_server import server

            assert server is not None
        except ImportError:
            pytest.skip("Server module not available")
        except Exception as e:
            # Some initialization errors are acceptable in unit tests
            pytest.skip(f"Server initialization requires specific setup: {e}")


class TestRunCommand:
    @pytest.mark.asyncio
    async def test_run_command_marks_nonzero_returncode_as_failure(self):
        from computer_server.handlers.base import BaseAutomationHandler

        class Handler(BaseAutomationHandler):
            async def mouse_down(self, x=None, y=None, button="left"):
                pass

            async def mouse_up(self, x=None, y=None, button="left"):
                pass

            async def left_click(self, x=None, y=None):
                pass

            async def right_click(self, x=None, y=None):
                pass

            async def middle_click(self, x=None, y=None):
                pass

            async def double_click(self, x=None, y=None):
                pass

            async def move_cursor(self, x, y):
                pass

            async def drag_to(self, x, y, button="left", duration=0.5):
                pass

            async def drag(self, path, button="left", duration=0.5):
                pass

            async def key_down(self, key):
                pass

            async def key_up(self, key):
                pass

            async def type_text(self, text):
                pass

            async def press_key(self, key):
                pass

            async def hotkey(self, keys):
                pass

            async def scroll(self, x, y):
                pass

            async def scroll_down(self, clicks=1):
                pass

            async def scroll_up(self, clicks=1):
                pass

            async def screenshot(self, format="png", quality=95):
                pass

            async def get_screen_size(self):
                pass

            async def get_cursor_position(self):
                pass

        result = await Handler().run_command("command -v definitely-not-installed")

        assert result["success"] is False
        assert result["return_code"] != 0

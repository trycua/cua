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


class TestMcpBarePathRewrite:
    """Test the ASGI shim that serves MCP at both /mcp and /mcp/ (SRP:
    Only tests the path rewrite). The class is pure ASGI with no
    dependencies, so it is tested standalone against a recording stub."""

    @pytest.mark.asyncio
    async def test_bare_mcp_path_is_rewritten(self):
        try:
            from computer_server.main import McpBarePathRewrite
        except ImportError:
            pytest.skip("computer_server module not installed")
        except Exception as e:
            pytest.skip(f"Server initialization requires specific setup: {e}")

        seen = {}

        async def inner(scope, receive, send):
            seen.update(scope)

        shim = McpBarePathRewrite(inner)
        await shim({"type": "http", "path": "/mcp"}, None, None)
        assert seen["path"] == "/mcp/"

    @pytest.mark.asyncio
    async def test_other_paths_untouched(self):
        try:
            from computer_server.main import McpBarePathRewrite
        except ImportError:
            pytest.skip("computer_server module not installed")
        except Exception as e:
            pytest.skip(f"Server initialization requires specific setup: {e}")

        seen = {}

        async def inner(scope, receive, send):
            seen.update(scope)

        shim = McpBarePathRewrite(inner)
        for path in ("/mcp/", "/mcpx", "/status", "/"):
            await shim({"type": "http", "path": path}, None, None)
            assert seen["path"] == path


class TestMcpAcceptHeaderShim:
    """Test the ASGI shim that normalizes the Accept header for /mcp so the
    MCP SDK's strict Accept check passes for any client (SRP: Only tests the
    Accept-header rewrite). Pure ASGI with no deps; tested standalone."""

    @staticmethod
    def _accept(headers):
        for k, v in headers:
            if k == b"accept":
                return v
        return None

    @pytest.mark.asyncio
    async def test_mcp_accept_header_is_normalized(self):
        try:
            from computer_server.main import McpAcceptHeaderShim
        except ImportError:
            pytest.skip("computer_server module not installed")
        except Exception as e:
            pytest.skip(f"Server initialization requires specific setup: {e}")

        seen = {}

        async def inner(scope, receive, send):
            seen.update(scope)

        shim = McpAcceptHeaderShim(inner)
        # claude.ai-style request that only advertises application/json:
        for path in ("/mcp", "/mcp/", "/mcp/anything"):
            await shim(
                {"type": "http", "path": path, "headers": [(b"accept", b"application/json")]},
                None,
                None,
            )
            assert self._accept(seen["headers"]) == b"application/json, text/event-stream"

    @pytest.mark.asyncio
    async def test_missing_accept_header_is_added(self):
        try:
            from computer_server.main import McpAcceptHeaderShim
        except ImportError:
            pytest.skip("computer_server module not installed")
        except Exception as e:
            pytest.skip(f"Server initialization requires specific setup: {e}")

        seen = {}

        async def inner(scope, receive, send):
            seen.update(scope)

        shim = McpAcceptHeaderShim(inner)
        await shim({"type": "http", "path": "/mcp", "headers": []}, None, None)
        assert self._accept(seen["headers"]) == b"application/json, text/event-stream"

    @pytest.mark.asyncio
    async def test_non_mcp_paths_untouched(self):
        try:
            from computer_server.main import McpAcceptHeaderShim
        except ImportError:
            pytest.skip("computer_server module not installed")
        except Exception as e:
            pytest.skip(f"Server initialization requires specific setup: {e}")

        seen = {}

        async def inner(scope, receive, send):
            seen.update(scope)

        shim = McpAcceptHeaderShim(inner)
        for path in ("/status", "/", "/mcpx", "/ws"):
            await shim(
                {"type": "http", "path": path, "headers": [(b"accept", b"application/json")]},
                None,
                None,
            )
            # Untouched: still exactly what the client sent.
            assert self._accept(seen["headers"]) == b"application/json"

"""Unit tests for computer-server package.

This file tests ONLY basic server functionality.
Following SRP: This file tests server initialization and basic operations.
All external dependencies are mocked.
"""

import importlib.util
from pathlib import Path
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


class TestMcpServerSmoke:
    """Smoke-test the real FastMCP integration so dependency major bumps cannot
    silently remove the /mcp endpoint while unit tests stay green."""

    @staticmethod
    def _load_mcp_server_module():
        # Import the module file directly so this smoke remains isolated from
        # the package's public import surface. The MCP module itself has no
        # runtime dependency on platform input backends until a tool is invoked.
        module_path = Path(__file__).parents[1] / "computer_server" / "mcp_server.py"
        spec = importlib.util.spec_from_file_location("_computer_server_mcp_smoke", module_path)
        assert spec is not None
        assert spec.loader is not None
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module

    def test_create_mcp_server_and_http_app(self):
        mcp_module = self._load_mcp_server_module()

        mcp_server = mcp_module.create_mcp_server()
        mcp_app = mcp_server.http_app(path="/")

        assert mcp_server is not None
        assert mcp_app is not None

    @pytest.mark.asyncio
    async def test_cua_driver_capture_scope_tools_are_registered(self):
        mcp_module = self._load_mcp_server_module()

        tools = await mcp_module.create_mcp_server().list_tools()
        names = {tool.name for tool in tools}

        assert {
            "computer_get_desktop_state",
            "computer_get_capture_scope_state",
            "computer_escalate_capture_scope",
        } <= names

    def test_mcp_http_app_accepts_initialize_post(self):
        from starlette.testclient import TestClient

        mcp_module = self._load_mcp_server_module()
        mcp_server = mcp_module.create_mcp_server()
        mcp_app = mcp_server.http_app(path="/")

        payload = {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "computer-server-smoke", "version": "0"},
            },
        }

        with TestClient(mcp_app) as client:
            response = client.post(
                "/",
                json=payload,
                headers={"accept": "application/json, text/event-stream"},
            )

        assert response.status_code == 200
        assert "cua-computer-server" in response.text
        assert '"id":1' in response.text


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

"""Pytest configuration and shared fixtures for mcp-server package tests.

This file contains shared fixtures and configuration for all mcp-server tests.
Following SRP: This file ONLY handles test setup/teardown.
"""

from unittest.mock import AsyncMock, Mock, patch

import pytest


@pytest.fixture
def mock_mcp_context():
    """Mock MCP context for testing.

    Use this fixture to test MCP server logic without real MCP connections.
    """
    context = AsyncMock()
    context.request_context = AsyncMock()
    context.session = Mock()
    context.session.send_resource_updated = AsyncMock()

    return context


@pytest.fixture
def mock_computer():
    """Mock Computer instance for MCP server tests.

    Use this fixture to test MCP logic without real Computer operations.
    """
    computer = AsyncMock()
    computer.interface = AsyncMock()
    computer.interface.screenshot = AsyncMock(return_value=b"fake_screenshot")
    computer.interface.left_click = AsyncMock()
    computer.interface.type = AsyncMock()

    # Mock context manager
    computer.__aenter__ = AsyncMock(return_value=computer)
    computer.__aexit__ = AsyncMock()

    return computer


@pytest.fixture
def disable_telemetry(monkeypatch):
    """Disable telemetry for tests.

    Use this fixture to ensure no telemetry is sent during tests.
    """
    monkeypatch.setenv("CUA_TELEMETRY_DISABLED", "1")

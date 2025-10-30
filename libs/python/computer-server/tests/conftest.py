"""Pytest configuration and shared fixtures for computer-server package tests.

This file contains shared fixtures and configuration for all computer-server tests.
Following SRP: This file ONLY handles test setup/teardown.
"""

from unittest.mock import AsyncMock, Mock, patch

import pytest


@pytest.fixture
def mock_websocket():
    """Mock WebSocket connection for testing.

    Use this fixture to test WebSocket logic without real connections.
    """
    websocket = AsyncMock()
    websocket.send = AsyncMock()
    websocket.recv = AsyncMock()
    websocket.close = AsyncMock()

    return websocket


@pytest.fixture
def mock_computer_interface():
    """Mock computer interface for server tests.

    Use this fixture to test server logic without real computer operations.
    """
    interface = AsyncMock()
    interface.screenshot = AsyncMock(return_value=b"fake_screenshot")
    interface.left_click = AsyncMock()
    interface.type = AsyncMock()
    interface.key = AsyncMock()

    return interface


@pytest.fixture
def disable_telemetry(monkeypatch):
    """Disable telemetry for tests.

    Use this fixture to ensure no telemetry is sent during tests.
    """
    monkeypatch.setenv("CUA_TELEMETRY_DISABLED", "1")

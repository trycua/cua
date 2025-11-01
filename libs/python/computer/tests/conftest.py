"""Pytest configuration and shared fixtures for computer package tests.

This file contains shared fixtures and configuration for all computer tests.
Following SRP: This file ONLY handles test setup/teardown.
"""

from unittest.mock import AsyncMock, MagicMock, Mock, patch

import pytest


@pytest.fixture
def mock_interface():
    """Mock computer interface for testing.

    Use this fixture to test Computer logic without real OS calls.
    """
    interface = AsyncMock()
    interface.screenshot = AsyncMock(return_value=b"fake_screenshot")
    interface.left_click = AsyncMock()
    interface.right_click = AsyncMock()
    interface.middle_click = AsyncMock()
    interface.double_click = AsyncMock()
    interface.type = AsyncMock()
    interface.key = AsyncMock()
    interface.move_mouse = AsyncMock()
    interface.scroll = AsyncMock()
    interface.get_screen_size = AsyncMock(return_value=(1920, 1080))

    return interface


@pytest.fixture
def mock_cloud_provider():
    """Mock cloud provider for testing.

    Use this fixture to test cloud provider logic without real API calls.
    """
    provider = AsyncMock()
    provider.start = AsyncMock()
    provider.stop = AsyncMock()
    provider.get_status = AsyncMock(return_value="running")
    provider.execute_command = AsyncMock(return_value="command output")

    return provider


@pytest.fixture
def mock_local_provider():
    """Mock local provider for testing.

    Use this fixture to test local provider logic without real VM operations.
    """
    provider = AsyncMock()
    provider.start = AsyncMock()
    provider.stop = AsyncMock()
    provider.get_status = AsyncMock(return_value="running")
    provider.execute_command = AsyncMock(return_value="command output")

    return provider


@pytest.fixture
def disable_telemetry(monkeypatch):
    """Disable telemetry for tests.

    Use this fixture to ensure no telemetry is sent during tests.
    """
    monkeypatch.setenv("CUA_TELEMETRY_DISABLED", "1")

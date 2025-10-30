"""Pytest configuration and shared fixtures for core package tests.

This file contains shared fixtures and configuration for all core tests.
Following SRP: This file ONLY handles test setup/teardown.
"""

from unittest.mock import AsyncMock, Mock, patch

import pytest


@pytest.fixture
def mock_httpx_client():
    """Mock httpx.AsyncClient for API calls.

    Use this fixture to avoid making real HTTP requests during tests.
    """
    with patch("httpx.AsyncClient") as mock_client:
        mock_instance = AsyncMock()
        mock_client.return_value.__aenter__.return_value = mock_instance
        yield mock_instance


@pytest.fixture
def mock_posthog():
    """Mock PostHog client for telemetry tests.

    Use this fixture to avoid sending real telemetry during tests.
    """
    with patch("posthog.Posthog") as mock_ph:
        mock_instance = Mock()
        mock_ph.return_value = mock_instance
        yield mock_instance


@pytest.fixture
def disable_telemetry(monkeypatch):
    """Disable telemetry for tests that don't need it.

    Use this fixture to ensure telemetry is disabled during tests.
    """
    monkeypatch.setenv("CUA_TELEMETRY_DISABLED", "1")
    yield

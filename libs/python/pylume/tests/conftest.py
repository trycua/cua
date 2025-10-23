"""Pytest configuration and shared fixtures for pylume package tests.

This file contains shared fixtures and configuration for all pylume tests.
Following SRP: This file ONLY handles test setup/teardown.
"""

import pytest
from unittest.mock import Mock, AsyncMock, patch


@pytest.fixture
def mock_subprocess():
    """Mock subprocess calls for testing.
    
    Use this fixture to test command execution without running real commands.
    """
    with patch("subprocess.run") as mock_run:
        mock_run.return_value = Mock(
            returncode=0,
            stdout="mocked output",
            stderr=""
        )
        yield mock_run


@pytest.fixture
def mock_lume_cli():
    """Mock Lume CLI interactions.
    
    Use this fixture to test Lume integration without real VM operations.
    """
    with patch("pylume.lume.LumeClient") as mock_client:
        mock_instance = Mock()
        mock_instance.list_vms = Mock(return_value=[])
        mock_instance.create_vm = Mock(return_value={"id": "test-vm-123"})
        mock_instance.delete_vm = Mock(return_value=True)
        mock_client.return_value = mock_instance
        yield mock_instance


@pytest.fixture
def disable_telemetry(monkeypatch):
    """Disable telemetry for tests.
    
    Use this fixture to ensure no telemetry is sent during tests.
    """
    monkeypatch.setenv("CUA_TELEMETRY_DISABLED", "1")

"""Pytest configuration for pylume tests.

This module provides test fixtures for the pylume package.
Note: This package has macOS-specific dependencies and will skip tests
if the required modules are not available.
"""

from unittest.mock import Mock, patch

import pytest


@pytest.fixture
def mock_subprocess():
    with patch("subprocess.run") as mock_run:
        mock_run.return_value = Mock(returncode=0, stdout="", stderr="")
        yield mock_run


@pytest.fixture
def mock_requests():
    with patch("requests.get") as mock_get, patch("requests.post") as mock_post:
        yield {"get": mock_get, "post": mock_post}

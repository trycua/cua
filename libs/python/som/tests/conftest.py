"""Pytest configuration for som tests.

This module provides test fixtures for the som (Set-of-Mark) package.
The som package depends on heavy ML models and will skip tests if not available.
"""

from unittest.mock import Mock, patch

import pytest


@pytest.fixture
def mock_torch():
    with patch("torch.load") as mock_load:
        mock_load.return_value = Mock()
        yield mock_load


@pytest.fixture
def mock_icon_detector():
    with patch("omniparser.IconDetector") as mock_detector:
        instance = Mock()
        mock_detector.return_value = instance
        yield instance

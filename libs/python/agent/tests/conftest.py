"""Pytest configuration and shared fixtures for agent package tests.

This file contains shared fixtures and configuration for all agent tests.
Following SRP: This file ONLY handles test setup/teardown.
"""

from unittest.mock import AsyncMock, MagicMock, Mock, patch

import pytest


@pytest.fixture
def mock_litellm():
    """Mock liteLLM completion calls.

    Use this fixture to avoid making real LLM API calls during tests.
    Returns a mock that simulates LLM responses.
    """
    with patch("litellm.acompletion") as mock_completion:

        async def mock_response(*args, **kwargs):
            """Simulate a typical LLM response."""
            return {
                "id": "chatcmpl-test123",
                "object": "chat.completion",
                "created": 1234567890,
                "model": kwargs.get("model", "anthropic/claude-3-5-sonnet-20241022"),
                "choices": [
                    {
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": "This is a mocked response for testing.",
                        },
                        "finish_reason": "stop",
                    }
                ],
                "usage": {
                    "prompt_tokens": 10,
                    "completion_tokens": 20,
                    "total_tokens": 30,
                },
            }

        mock_completion.side_effect = mock_response
        yield mock_completion


@pytest.fixture
def mock_computer():
    """Mock Computer interface for agent tests.

    Use this fixture to test agent logic without requiring a real Computer instance.
    """
    computer = AsyncMock()
    computer.interface = AsyncMock()
    computer.interface.screenshot = AsyncMock(return_value=b"fake_screenshot_data")
    computer.interface.left_click = AsyncMock()
    computer.interface.type = AsyncMock()
    computer.interface.key = AsyncMock()

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


@pytest.fixture
def sample_messages():
    """Provide sample messages for testing.

    Returns a list of messages in the expected format.
    """
    return [{"role": "user", "content": "Take a screenshot and tell me what you see"}]

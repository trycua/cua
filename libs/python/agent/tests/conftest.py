"""Pytest configuration and shared fixtures for agent package tests.

This file contains shared fixtures and configuration for all agent tests.
Following SRP: This file ONLY handles test setup/teardown.

Windows / Anaconda note (issue #1381)
--------------------------------------
On Windows with Anaconda, importing torch before numpy (or vice-versa) can
load two copies of the OpenMP runtime (libiomp5md.dll / vcomp*.dll), which
causes a fatal ``OMP: Error #15`` crash.  The two-line environment-variable
patch below sets KMP_DUPLICATE_LIB_OK=TRUE *before* any test module imports
torch, which silences the crash.  This is a well-known Intel MKL/OMP
workaround and is safe for a test environment.
"""

import os
import sys

# ── Windows / Anaconda OMP duplicate-library fix (issue #1381) ────────────
# Must be set before torch is imported by any test or fixture.
os.environ.setdefault("KMP_DUPLICATE_LIB_OK", "TRUE")
# ──────────────────────────────────────────────────────────────────────────

from unittest.mock import AsyncMock, MagicMock, Mock, patch

import pytest


# ---------------------------------------------------------------------------
# Torch / Transformers mock fixture
# ---------------------------------------------------------------------------


@pytest.fixture(scope="session", autouse=True)
def mock_torch_and_transformers():
    """Session-scoped fixture that injects fake torch/transformers modules.

    Injecting lightweight stubs into sys.modules prevents the real PyTorch
    from being imported during the test session.  This avoids:

    * The OMP duplicate-library fatal crash on Windows + Anaconda (#1381)
    * Long import times / CUDA initialisation in CI without a GPU
    * Tests accidentally requiring heavyweight ML dependencies

    The stubs expose just enough surface area so that
    ``huggingfacelocal_adapter.py`` (and ``adapters/models/__init__.py``)
    can be imported without error.
    """
    from types import ModuleType

    def _stub(name: str, **attrs) -> ModuleType:
        m = ModuleType(name)
        for k, v in attrs.items():
            setattr(m, k, v)
        return m

    # torch stub
    torch_stub = _stub(
        "torch",
        cuda=MagicMock(is_available=Mock(return_value=False)),
        device=Mock,
        float16=None,
        bfloat16=None,
    )

    # transformers stub
    transformers_stub = _stub(
        "transformers",
        AutoModelForImageTextToText=MagicMock(),
        AutoProcessor=MagicMock(),
        AutoConfig=MagicMock(),
    )

    stubs = {
        "torch": torch_stub,
        "transformers": transformers_stub,
    }

    originals = {k: sys.modules.get(k) for k in stubs}

    for name, stub in stubs.items():
        existing = sys.modules.get(name)
        if existing is None:
            sys.modules[name] = stub
        else:
            # Fill in any attributes the earlier stub didn't define so that
            # session-wide stubs remain authoritative even when a test module
            # already inserted a minimal stub via sys.modules.setdefault().
            for attr in vars(stub):
                if not hasattr(existing, attr):
                    setattr(existing, attr, getattr(stub, attr))

    yield

    # Restore originals (important when running alongside integration tests)
    for name, original in originals.items():
        if original is None:
            sys.modules.pop(name, None)
        else:
            sys.modules[name] = original


# ---------------------------------------------------------------------------
# Existing shared fixtures (unchanged)
# ---------------------------------------------------------------------------


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
                "model": kwargs.get("model", "anthropic/claude-sonnet-4-5-20250929"),
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
    monkeypatch.setenv("CUA_TELEMETRY_ENABLED", "false")


@pytest.fixture
def sample_messages():
    """Provide sample messages for testing.

    Returns a list of messages in the expected format.
    """
    return [{"role": "user", "content": "Take a screenshot and tell me what you see"}]

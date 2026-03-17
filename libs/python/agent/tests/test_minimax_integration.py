"""Integration tests for MiniMax adapter with real API calls.

These tests require MINIMAX_API_KEY to be set.
Run with: pytest tests/test_minimax_integration.py -v

Skipped automatically if MINIMAX_API_KEY is not available.
"""

import os

import pytest

from agent.adapters.minimax_adapter import MiniMaxAdapter

pytestmark = pytest.mark.skipif(
    not os.environ.get("MINIMAX_API_KEY"),
    reason="MINIMAX_API_KEY not set",
)


class TestMiniMaxLiveCompletion:
    """Integration tests with the MiniMax API."""

    def test_sync_completion(self):
        """Test synchronous completion with MiniMax M2.5."""
        adapter = MiniMaxAdapter()
        response = adapter.completion(
            model="minimax/MiniMax-M2.5",
            messages=[{"role": "user", "content": "Say hello in one word."}],
            temperature=0.7,
            max_tokens=10,
        )
        assert response is not None
        assert hasattr(response, "choices") or isinstance(response, dict)

    @pytest.mark.asyncio
    async def test_async_completion(self):
        """Test async completion with MiniMax M2.5."""
        adapter = MiniMaxAdapter()
        response = await adapter.acompletion(
            model="minimax/MiniMax-M2.5",
            messages=[{"role": "user", "content": "Say hello in one word."}],
            temperature=0.7,
            max_tokens=10,
        )
        assert response is not None

    def test_highspeed_model(self):
        """Test completion with MiniMax M2.5 highspeed variant."""
        adapter = MiniMaxAdapter()
        response = adapter.completion(
            model="minimax/MiniMax-M2.5-highspeed",
            messages=[{"role": "user", "content": "Say hi."}],
            max_tokens=10,
        )
        assert response is not None

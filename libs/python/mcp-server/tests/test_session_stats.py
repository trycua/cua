"""Unit tests for SessionManager.get_session_stats."""

import asyncio
import threading
from unittest.mock import AsyncMock

import pytest
from mcp_server.session_manager import SessionManager


@pytest.mark.asyncio
async def test_get_session_stats_inside_running_loop():
    """The async MCP tool calls get_session_stats() on the server's own loop."""
    manager = SessionManager(max_concurrent_sessions=3)
    computer = AsyncMock()
    manager._computer_pool.acquire = AsyncMock(return_value=computer)
    async with manager.get_session("s1"):
        pass

    result = {}

    # Run on a worker thread's own event loop so a hang fails the test
    # instead of freezing pytest.
    def call_in_loop():
        async def tool():
            result["stats"] = manager.get_session_stats()

        asyncio.run(tool())

    worker = threading.Thread(target=call_in_loop, daemon=True)
    worker.start()
    worker.join(timeout=2)

    assert not worker.is_alive(), "get_session_stats() blocked the running event loop"
    stats = result["stats"]
    assert stats["total_sessions"] == 1
    assert stats["max_concurrent"] == 3
    assert stats["sessions"]["s1"]["active_tasks"] == 0


def test_get_session_stats_without_running_loop():
    manager = SessionManager()
    assert manager.get_session_stats()["total_sessions"] == 0

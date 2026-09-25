"""Unit tests for the mcp-server SessionManager lifecycle."""

import asyncio
from unittest.mock import AsyncMock

import pytest
from mcp_server.session_manager import SessionManager


async def _manager_with_session(session_id: str):
    manager = SessionManager()
    computer = AsyncMock()
    manager._computer_pool.acquire = AsyncMock(return_value=computer)
    manager._computer_pool._in_use.add(computer)
    async with manager.get_session(session_id):
        pass
    return manager, computer


@pytest.mark.asyncio
async def test_cleanup_session_releases_idle_session():
    manager, computer = await _manager_with_session("s1")

    await asyncio.wait_for(manager.cleanup_session("s1"), timeout=2)

    assert "s1" not in manager._sessions
    assert computer in manager._computer_pool._available
    assert not manager._session_lock.locked()


@pytest.mark.asyncio
async def test_cleanup_session_defers_while_tasks_are_active():
    manager, computer = await _manager_with_session("s1")
    await manager.register_task("s1", "task-1")

    await asyncio.wait_for(manager.cleanup_session("s1"), timeout=2)

    assert manager._sessions["s1"].is_shutting_down
    assert computer not in manager._computer_pool._available

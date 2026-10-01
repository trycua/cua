"""Unit tests for the mcp-server SessionManager lifecycle."""

import asyncio
from unittest.mock import AsyncMock

import pytest
from mcp_server.session_manager import SessionInfo, SessionManager


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


@pytest.mark.asyncio
async def test_new_session_waiting_for_pool_does_not_block_cleanup():
    manager = SessionManager()
    pool = manager._computer_pool
    pool.max_size = 1
    computer = AsyncMock()
    pool._in_use.add(computer)
    # Session "s1" holds the only computer in the pool.
    manager._sessions["s1"] = SessionInfo(
        session_id="s1", computer=computer, created_at=0.0, last_activity=0.0
    )

    async def open_second_session():
        async with manager.get_session("s2") as session:
            return session.computer

    waiter = asyncio.create_task(open_second_session())
    await asyncio.sleep(0.3)  # "s2" is now waiting for a free computer
    assert not waiter.done()

    # Freeing "s1" must still be possible, and it hands the computer to "s2".
    await asyncio.wait_for(manager.cleanup_session("s1"), timeout=2)
    assert await asyncio.wait_for(waiter, timeout=2) is computer
    assert set(manager._sessions) == {"s2"}

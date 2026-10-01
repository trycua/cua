"""Unit tests for PtyManager in computer-server."""

import asyncio
from unittest.mock import MagicMock

import pytest

from computer_server.pty_manager import PtyManager


@pytest.mark.asyncio
async def test_pty_manager_flushes_early_buffer_on_subscribe():
    """Test that early output chunks produced before PID assignment are delivered to subscribers on subscribe()."""
    mgr = PtyManager()

    # Mock terminal instance
    mock_terminal = MagicMock()
    mock_session = MagicMock()
    mock_session.pid = 42
    mock_session.cols = 80
    mock_session.rows = 24

    def fake_create(command, cols, rows, on_data, cwd, envs):
        # Simulate early output callback firing during session creation before PID return
        on_data(b"EARLY_PROMPT> ")
        on_data(b"WELCOME BANNER\n")
        return mock_session

    mock_terminal.create.side_effect = fake_create
    mgr._terminal = mock_terminal

    # 1. Create session (early output is collected in _early_buffers[42])
    info = await mgr.create(command="bash", cols=80, rows=24)
    assert info["pid"] == 42

    # 2. Subscribe to output (subscribe should deliver all early output chunks)
    q = mgr.subscribe(42)

    # 3. Verify early output chunks were queued losslessly
    assert not q.empty()
    msg1 = q.get_nowait()
    assert msg1 == {"type": "output", "data": b"EARLY_PROMPT> "}

    msg2 = q.get_nowait()
    assert msg2 == {"type": "output", "data": b"WELCOME BANNER\n"}


@pytest.mark.asyncio
async def test_pty_manager_send_stdin_and_info():
    """Test send_stdin and get_info functionality."""
    mgr = PtyManager()
    mock_terminal = MagicMock()
    mock_session = MagicMock()
    mock_session.pid = 100
    mock_session.cols = 100
    mock_session.rows = 30
    mock_terminal.create.return_value = mock_session
    mgr._terminal = mock_terminal

    info = await mgr.create()
    assert info["pid"] == 100

    info_retrieved = mgr.get_info(100)
    assert info_retrieved == {"pid": 100, "cols": 100, "rows": 30}

    await mgr.send_stdin(100, b"ls\n")
    mock_terminal.send_stdin.assert_called_once_with(100, b"ls\n")


@pytest.mark.asyncio
async def test_pty_manager_kill_cleans_early_buffers():
    """Test kill removes early buffer and broadcasts exit sentinel."""
    mgr = PtyManager()
    mock_terminal = MagicMock()
    mock_session = MagicMock()
    mock_session.pid = 200
    mock_session.cols = 80
    mock_session.rows = 24
    mock_terminal.create.return_value = mock_session
    mock_terminal.kill.return_value = True
    mgr._terminal = mock_terminal

    await mgr.create()
    q = mgr.subscribe(200)

    result = await mgr.kill(200)
    assert result is True
    assert 200 not in mgr._early_buffers

    msg = q.get_nowait()
    assert msg == {"type": "exit", "code": -1}

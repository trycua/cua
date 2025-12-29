"""
Tests for MCP Server Session Management functionality.

This module tests the new concurrent session management and resource lifecycle features.
"""

import asyncio
import importlib.util
import sys
import time
import types
from pathlib import Path

import pytest


def _install_stub_module(
    name: str, module: types.ModuleType, registry: dict[str, types.ModuleType | None]
) -> None:
    registry[name] = sys.modules.get(name)
    sys.modules[name] = module


@pytest.fixture
def server_module():
    """Create a server module with stubbed dependencies for testing."""
    stubbed_modules: dict[str, types.ModuleType | None] = {}

    # Stub MCP Context primitives
    mcp_module = types.ModuleType("mcp")
    mcp_module.__path__ = []  # mark as package

    mcp_server_module = types.ModuleType("mcp.server")
    mcp_server_module.__path__ = []

    fastmcp_module = types.ModuleType("mcp.server.fastmcp")

    class _StubContext:
        async def yield_message(self, *args, **kwargs):
            return None

        async def yield_tool_call(self, *args, **kwargs):
            return None

        async def yield_tool_output(self, *args, **kwargs):
            return None

        def report_progress(self, *_args, **_kwargs):
            return None

        def info(self, *_args, **_kwargs):
            return None

        def error(self, *_args, **_kwargs):
            return None

    class _StubImage:
        def __init__(self, format: str, data: bytes):
            self.format = format
            self.data = data

    class _StubFastMCP:
        def __init__(self, name: str):
            self.name = name
            self._tools: dict[str, types.FunctionType] = {}

        def tool(self, *args, **kwargs):
            def decorator(func):
                self._tools[func.__name__] = func
                return func

            return decorator

        def run(self):
            return None

    fastmcp_module.Context = _StubContext
    fastmcp_module.FastMCP = _StubFastMCP
    fastmcp_module.Image = _StubImage

    _install_stub_module("mcp", mcp_module, stubbed_modules)
    _install_stub_module("mcp.server", mcp_server_module, stubbed_modules)
    _install_stub_module("mcp.server.fastmcp", fastmcp_module, stubbed_modules)

    # Stub Computer module
    computer_module = types.ModuleType("computer")

    class _StubInterface:
        async def screenshot(self) -> bytes:
            return b"test-screenshot-data"

    class _StubComputer:
        def __init__(self, *args, **kwargs):
            self.interface = _StubInterface()

        async def run(self):
            return None

    computer_module.Computer = _StubComputer

    _install_stub_module("computer", computer_module, stubbed_modules)

    # Stub agent module
    agent_module = types.ModuleType("agent")

    class _StubComputerAgent:
        def __init__(self, *args, **kwargs):
            pass

        async def run(self, *_args, **_kwargs):
            # Simulate agent execution with streaming
            yield {
                "output": [
                    {
                        "type": "message",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": "Task completed"}],
                    }
                ]
            }

    agent_module.ComputerAgent = _StubComputerAgent

    _install_stub_module("agent", agent_module, stubbed_modules)

    # Stub session manager module
    session_manager_module = types.ModuleType("mcp_server.session_manager")

    class _StubSessionInfo:
        def __init__(self, session_id: str, computer, created_at: float, last_activity: float):
            self.session_id = session_id
            self.computer = computer
            self.created_at = created_at
            self.last_activity = last_activity
            self.active_tasks = set()
            self.is_shutting_down = False

    class _StubSessionManager:
        def __init__(self):
            self._sessions = {}
            self._session_lock = asyncio.Lock()

        async def get_session(self, session_id=None):
            """Context manager that returns a session."""
            if session_id is None:
                session_id = "test-session-123"

            async with self._session_lock:
                if session_id not in self._sessions:
                    computer = _StubComputer()
                    session = _StubSessionInfo(
                        session_id=session_id,
                        computer=computer,
                        created_at=time.time(),
                        last_activity=time.time(),
                    )
                    self._sessions[session_id] = session

                return self._sessions[session_id]

        async def register_task(self, session_id: str, task_id: str):
            pass

        async def unregister_task(self, session_id: str, task_id: str):
            pass

        async def cleanup_session(self, session_id: str):
            async with self._session_lock:
                self._sessions.pop(session_id, None)

        def get_session_stats(self):
            return {
                "total_sessions": len(self._sessions),
                "max_concurrent": 10,
                "sessions": {sid: {"active_tasks": 0} for sid in self._sessions},
            }

    _stub_session_manager = _StubSessionManager()

    def get_session_manager():
        return _stub_session_manager

    async def initialize_session_manager():
        return _stub_session_manager

    async def shutdown_session_manager():
        pass

    session_manager_module.get_session_manager = get_session_manager
    session_manager_module.initialize_session_manager = initialize_session_manager
    session_manager_module.shutdown_session_manager = shutdown_session_manager

    _install_stub_module("mcp_server.session_manager", session_manager_module, stubbed_modules)

    # Load the actual server module
    module_name = "mcp_server_server_under_test"
    module_path = Path("libs/python/mcp-server/mcp_server/server.py").resolve()
    spec = importlib.util.spec_from_file_location(module_name, module_path)
    server_module = importlib.util.module_from_spec(spec)
    assert spec and spec.loader
    spec.loader.exec_module(server_module)

    server_instance = getattr(server_module, "server", None)
    if server_instance is not None and hasattr(server_instance, "_tools"):
        for name, func in server_instance._tools.items():
            setattr(server_module, name, func)

    try:
        yield server_module
    finally:
        sys.modules.pop(module_name, None)
        for name, original in stubbed_modules.items():
            if original is None:
                sys.modules.pop(name, None)
            else:
                sys.modules[name] = original


class FakeContext:
    """Fake context for testing."""

    def __init__(self) -> None:
        self.events: list[tuple] = []
        self.progress_updates: list[float] = []

    def info(self, message: str) -> None:
        self.events.append(("info", message))

    def error(self, message: str) -> None:
        self.events.append(("error", message))

    def report_progress(self, value: float) -> None:
        self.progress_updates.append(value)

    async def yield_message(self, *, role: str, content):
        timestamp = asyncio.get_running_loop().time()
        self.events.append(("message", role, content, timestamp))

    async def yield_tool_call(self, *, name: str | None, call_id: str, input):
        timestamp = asyncio.get_running_loop().time()
        self.events.append(("tool_call", name, call_id, input, timestamp))

    async def yield_tool_output(self, *, call_id: str, output, is_error: bool = False):
        timestamp = asyncio.get_running_loop().time()
        self.events.append(("tool_output", call_id, output, is_error, timestamp))


def test_screenshot_cua_with_session_id(server_module):
    """Test that screenshot_cua works with session management."""

    async def _run_test():
        ctx = FakeContext()
        result = await server_module.screenshot_cua(ctx, session_id="test-session")

        assert result.format == "png"
        assert result.data == b"test-screenshot-data"

    asyncio.run(_run_test())


def test_screenshot_cua_creates_new_session(server_module):
    """Test that screenshot_cua creates a new session when none provided."""

    async def _run_test():
        ctx = FakeContext()
        result = await server_module.screenshot_cua(ctx)

        assert result.format == "png"
        assert result.data == b"test-screenshot-data"

    asyncio.run(_run_test())


def test_run_cua_task_with_session_management(server_module):
    """Test that run_cua_task works with session management."""

    async def _run_test():
        ctx = FakeContext()
        task = "Test task"
        session_id = "test-session-456"

        text_result, image = await server_module.run_cua_task(ctx, task, session_id)

        assert "Task completed" in text_result
        assert image.format == "png"
        assert image.data == b"test-screenshot-data"

    asyncio.run(_run_test())


def test_run_multi_cua_tasks_sequential(server_module):
    """Test that run_multi_cua_tasks works sequentially."""

    async def _run_test():
        ctx = FakeContext()
        tasks = ["Task 1", "Task 2", "Task 3"]

        results = await server_module.run_multi_cua_tasks(ctx, tasks, concurrent=False)

        assert len(results) == 3
        for i, (text, image) in enumerate(results):
            assert "Task completed" in text
            assert image.format == "png"

    asyncio.run(_run_test())


def test_run_multi_cua_tasks_concurrent(server_module):
    """Test that run_multi_cua_tasks works concurrently."""

    async def _run_test():
        ctx = FakeContext()
        tasks = ["Task 1", "Task 2", "Task 3"]

        results = await server_module.run_multi_cua_tasks(ctx, tasks, concurrent=True)

        assert len(results) == 3
        for i, (text, image) in enumerate(results):
            assert "Task completed" in text
            assert image.format == "png"

    asyncio.run(_run_test())


def test_get_session_stats(server_module):
    """Test that get_session_stats returns proper statistics."""

    async def _run_test():
        ctx = FakeContext()
        stats = await server_module.get_session_stats()

        assert "total_sessions" in stats
        assert "max_concurrent" in stats
        assert "sessions" in stats

    asyncio.run(_run_test())


def test_cleanup_session(server_module):
    """Test that cleanup_session works properly."""

    async def _run_test():
        ctx = FakeContext()
        session_id = "test-cleanup-session"

        result = await server_module.cleanup_session(ctx, session_id)

        assert f"Session {session_id} cleanup initiated" in result

    asyncio.run(_run_test())


def test_concurrent_sessions_isolation(server_module):
    """Test that concurrent sessions are properly isolated."""

    async def _run_test():
        ctx = FakeContext()

        # Run multiple tasks with different session IDs concurrently
        task1 = asyncio.create_task(
            server_module.run_cua_task(ctx, "Task for session 1", "session-1")
        )
        task2 = asyncio.create_task(
            server_module.run_cua_task(ctx, "Task for session 2", "session-2")
        )

        results = await asyncio.gather(task1, task2)

        assert len(results) == 2
        for text, image in results:
            assert "Task completed" in text
            assert image.format == "png"

    asyncio.run(_run_test())


def test_session_reuse_with_same_id(server_module):
    """Test that sessions are reused when the same session ID is provided."""

    async def _run_test():
        ctx = FakeContext()
        session_id = "reuse-session"

        # First call
        result1 = await server_module.screenshot_cua(ctx, session_id)

        # Second call with same session ID
        result2 = await server_module.screenshot_cua(ctx, session_id)

        assert result1.format == result2.format
        assert result1.data == result2.data

    asyncio.run(_run_test())


def test_error_handling_with_session_management(server_module):
    """Test that errors are handled properly with session management."""

    async def _run_test():
        # Mock an agent that raises an exception
        class _FailingAgent:
            def __init__(self, *args, **kwargs):
                pass

            async def run(self, *_args, **_kwargs):
                raise RuntimeError("Simulated agent failure")

        # Replace the ComputerAgent with our failing one
        original_agent = server_module.ComputerAgent
        server_module.ComputerAgent = _FailingAgent

        try:
            ctx = FakeContext()
            task = "This will fail"

            text_result, image = await server_module.run_cua_task(ctx, task, "error-session")

            assert "Error during task execution" in text_result
            assert image.format == "png"

        finally:
            # Restore original agent
            server_module.ComputerAgent = original_agent

    asyncio.run(_run_test())

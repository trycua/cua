import asyncio
import importlib.util
import sys
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

    # Stub Computer module to avoid heavy dependencies
    computer_module = types.ModuleType("computer")

    class _StubInterface:
        async def screenshot(self) -> bytes:  # pragma: no cover - default stub
            return b""

    class _StubComputer:
        def __init__(self, *args, **kwargs):
            self.interface = _StubInterface()

        async def run(self):  # pragma: no cover - default stub
            return None

    class _StubVMProviderType:
        CLOUD = "cloud"
        LOCAL = "local"

    computer_module.Computer = _StubComputer
    computer_module.VMProviderType = _StubVMProviderType

    _install_stub_module("computer", computer_module, stubbed_modules)

    # Stub agent module so server can import ComputerAgent
    agent_module = types.ModuleType("agent")

    class _StubComputerAgent:
        def __init__(self, *args, **kwargs):
            pass

        async def run(self, *_args, **_kwargs):  # pragma: no cover - default stub
            if False:  # pragma: no cover
                yield {}
            return

    agent_module.ComputerAgent = _StubComputerAgent

    _install_stub_module("agent", agent_module, stubbed_modules)

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


def test_run_cua_task_streams_partial_results(server_module):
    async def _run_test():
        class FakeAgent:
            script = []

            def __init__(self, *args, **kwargs):
                pass

            async def run(self, messages):  # type: ignore[override]
                for factory, delay in type(self).script:
                    yield factory(messages)
                    if delay:
                        await asyncio.sleep(delay)

        FakeAgent.script = [
            (
                lambda _messages: {
                    "output": [
                        {
                            "type": "message",
                            "role": "assistant",
                            "content": [{"type": "output_text", "text": "First chunk"}],
                        }
                    ]
                },
                0.0,
            ),
            (
                lambda _messages: {
                    "output": [
                        {
                            "type": "tool_use",
                            "id": "call_1",
                            "name": "computer",
                            "input": {"action": "click"},
                        },
                        {
                            "type": "computer_call_output",
                            "call_id": "call_1",
                            "output": [{"type": "text", "text": "Tool completed"}],
                        },
                    ]
                },
                0.05,
            ),
        ]

        class FakeInterface:
            def __init__(self) -> None:
                self.calls = 0

            async def screenshot(self) -> bytes:
                self.calls += 1
                return b"final-image"

        fake_interface = FakeInterface()
        server_module.global_computer = types.SimpleNamespace(interface=fake_interface)
        server_module.ComputerAgent = FakeAgent  # type: ignore[assignment]

        ctx = FakeContext()
        task = asyncio.create_task(server_module.run_cua_task(ctx, "open settings"))

        await asyncio.sleep(0.01)
        assert not task.done(), "Task should still be running to simulate long operation"
        message_events = [event for event in ctx.events if event[0] == "message"]
        assert message_events, "Expected message event before task completion"

        text_result, image = await task

        assert "First chunk" in text_result
        assert "Tool completed" in text_result
        assert image.data == b"final-image"
        assert fake_interface.calls == 1

        tool_call_events = [event for event in ctx.events if event[0] == "tool_call"]
        tool_output_events = [event for event in ctx.events if event[0] == "tool_output"]
        assert tool_call_events and tool_output_events
        assert tool_call_events[0][2] == "call_1"
        assert tool_output_events[0][1] == "call_1"

    asyncio.run(_run_test())


def test_run_multi_cua_tasks_reports_progress(server_module, monkeypatch):
    async def _run_test():
        class FakeAgent:
            script = []

            def __init__(self, *args, **kwargs):
                pass

            async def run(self, messages):  # type: ignore[override]
                for factory, delay in type(self).script:
                    yield factory(messages)
                    if delay:
                        await asyncio.sleep(delay)

        FakeAgent.script = [
            (
                lambda messages: {
                    "output": [
                        {
                            "type": "message",
                            "role": "assistant",
                            "content": [
                                {
                                    "type": "output_text",
                                    "text": f"Result for {messages[0].get('content')}",
                                }
                            ],
                        }
                    ]
                },
                0.0,
            )
        ]

        server_module.ComputerAgent = FakeAgent  # type: ignore[assignment]

        class FakeInterface:
            async def screenshot(self) -> bytes:
                return b"progress-image"

        server_module.global_computer = types.SimpleNamespace(interface=FakeInterface())

        ctx = FakeContext()

        results = await server_module.run_multi_cua_tasks(ctx, ["a", "b", "c"])

        assert len(results) == 3
        assert results[0][0] == "Result for a"
        assert ctx.progress_updates[0] == pytest.approx(0.0)
        assert ctx.progress_updates[-1] == pytest.approx(1.0)
        assert len(ctx.progress_updates) == 6

    asyncio.run(_run_test())

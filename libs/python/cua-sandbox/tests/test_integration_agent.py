"""Integration tests for agent handler adapters."""

from __future__ import annotations

import pytest
from cua_sandbox.agent import SandboxHandler, is_sandbox

pytestmark = pytest.mark.asyncio


class TestSandboxHandler:
    async def test_screenshot(self, env_sandbox):
        handler = SandboxHandler(env_sandbox)
        b64 = await handler.screenshot()
        assert isinstance(b64, str)
        assert len(b64) > 100

    async def test_get_environment(self, env_sandbox):
        handler = SandboxHandler(env_sandbox)
        env = await handler.get_environment()
        assert env in ("windows", "mac", "linux", "browser")

    async def test_get_dimensions(self, env_sandbox):
        handler = SandboxHandler(env_sandbox)
        w, h = await handler.get_dimensions()
        assert w > 0 and h > 0

    async def test_click(self, env_sandbox):
        handler = SandboxHandler(env_sandbox)
        await handler.click(100, 100)

    async def test_type(self, env_sandbox):
        handler = SandboxHandler(env_sandbox)
        await handler.type("test")

    async def test_keypress(self, env_sandbox):
        handler = SandboxHandler(env_sandbox)
        await handler.keypress(["ctrl", "a"])

    async def test_scroll(self, env_sandbox):
        handler = SandboxHandler(env_sandbox)
        await handler.scroll(100, 100, 0, 3)

    async def test_move(self, env_sandbox):
        handler = SandboxHandler(env_sandbox)
        await handler.move(200, 200)

    async def test_drag(self, env_sandbox):
        handler = SandboxHandler(env_sandbox)
        await handler.drag([{"x": 100, "y": 100}, {"x": 200, "y": 200}])

    async def test_wait(self, env_sandbox):
        handler = SandboxHandler(env_sandbox)
        await handler.wait(10)  # 10ms


class TestIsSandbox:
    async def test_sandbox_detected(self, env_sandbox):
        assert is_sandbox(env_sandbox)

    def test_other_not_detected(self):
        assert not is_sandbox("not a sandbox")
        assert not is_sandbox(42)

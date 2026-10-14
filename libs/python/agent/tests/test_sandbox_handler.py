"""Hermetic tests for the cua-sandbox computer handlers (no VM, no host control)."""

from unittest.mock import AsyncMock, MagicMock

import pytest
from cua_agent.computers.sandbox import (
    LazySandboxComputerHandler,
    SandboxComputerHandler,
)


def _fake_sandbox():
    sb = MagicMock()
    sb.mouse.click = AsyncMock()
    sb.screenshot_base64 = AsyncMock(return_value="b64")
    sb.name = "fake-sb"
    return sb


class _CM:
    def __init__(self, sb):
        self.sb, self.entered, self.exited = sb, 0, 0

    async def __aenter__(self):
        self.entered += 1
        return self.sb

    async def __aexit__(self, *exc):
        self.exited += 1


@pytest.mark.asyncio
async def test_sandbox_handler_delegates():
    sb = _fake_sandbox()
    handler = SandboxComputerHandler(sb)
    await handler.click(1, 2)
    sb.mouse.click.assert_awaited_once_with(1, 2, button="left")
    assert await handler.screenshot() == "b64"


@pytest.mark.asyncio
async def test_lazy_handler_opens_once_and_closes():
    sb = _fake_sandbox()
    cm = _CM(sb)
    handler = LazySandboxComputerHandler(lambda: cm)
    assert cm.entered == 0
    assert await handler.screenshot() == "b64"
    await handler.click(3, 4)
    assert cm.entered == 1
    assert handler._sandbox is sb
    await handler.close()
    assert cm.exited == 1


@pytest.mark.asyncio
async def test_make_computer_handler_accepts_lazy_handler():
    from cua_agent.computers import make_computer_handler

    handler = LazySandboxComputerHandler(lambda: _CM(_fake_sandbox()))
    assert await make_computer_handler(handler) is handler


@pytest.mark.asyncio
async def test_drag_uses_path_endpoints():
    sb = _fake_sandbox()
    sb.mouse.drag = AsyncMock()
    await SandboxComputerHandler(sb).drag([{"x": 1, "y": 2}, {"x": 5, "y": 5}, {"x": 3, "y": 4}])
    sb.mouse.drag.assert_awaited_once_with(1, 2, 3, 4)


def test_open_sandbox_rejects_removed_localhost_provider():
    from cua_agent.computers.sandbox import open_sandbox

    with pytest.raises(ValueError, match="cua-driver"):
        open_sandbox("localhost")


@pytest.mark.asyncio
@pytest.mark.parametrize("key", ["localhost", "use_host_computer_server"])
async def test_proxy_rejects_removed_host_control(key):
    from cua_agent.proxy.handlers import _open_computer

    with pytest.raises(ValueError, match="cua-driver"):
        await _open_computer({key: True})


def test_is_agent_computer_rejects_plain_objects():
    from cua_agent.computers import is_agent_computer

    assert not is_agent_computer("not a computer")
    assert is_agent_computer({"screenshot": lambda: b""})

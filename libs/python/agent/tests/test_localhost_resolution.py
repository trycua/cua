"""Tests that a cua_sandbox Localhost is resolved like a Sandbox by the agent."""

from unittest.mock import patch

import pytest
from cua_agent.computers import (
    SandboxComputerHandler,
    is_agent_computer,
    make_computer_handler,
)


class _FakeLocalhost:
    pass


def test_localhost_is_recognized_as_agent_computer():
    host = _FakeLocalhost()
    assert is_agent_computer(host) is False
    with patch("cua_agent.computers.cuaLocalhost", _FakeLocalhost):
        assert is_agent_computer(host) is True


@pytest.mark.asyncio
async def test_make_computer_handler_wraps_localhost():
    host = _FakeLocalhost()
    with patch("cua_agent.computers.cuaLocalhost", _FakeLocalhost):
        handler = await make_computer_handler(host)
    assert isinstance(handler, SandboxComputerHandler)
    assert handler._sandbox is host

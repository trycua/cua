"""Tests for SDK-configurable ``run_command`` timeout propagation.

The SDK's ``cua_sandbox.interfaces.shell.Shell.run(cmd, timeout=...)`` sends
the timeout as a ``/cmd`` param; ``computer_server.main``'s dispatcher
filters kwargs by the handler signature before forwarding.  These tests
cover the handler-side contract:

* handler accepts ``timeout`` as a float keyword
* ``timeout=None`` → waits indefinitely
* ``timeout=<float>`` → times out at expiry with a ``success=False`` result
  containing the standardised ``Command timed out after <t>s`` stderr and
  ``return_code=-1``

The Android / Windows variants share the same shape; the base handler is
exercised here because it exposes a POSIX subprocess without adb/emulator
state being required for a focused unit test.
"""

import asyncio

import pytest
from computer_server.handlers.base import BaseAutomationHandler


class _MinimalHandler(BaseAutomationHandler):
    """Concrete BaseAutomationHandler for exercising ``run_command`` only.

    We clear ``__abstractmethods__`` AFTER class creation (below) instead
    of stubbing every abstract method — the set changes as the interface
    grows and keeping a mirror of the full abstract surface here is pure
    maintenance overhead for a test that only touches ``run_command``.

    Note: setting ``__abstractmethods__ = frozenset()`` inside the class
    body does NOT work — Python's ``ABCMeta`` overwrites it during class
    creation.  The override must happen after the class statement.
    """

    pass


# Bypass ABC enforcement — ABCMeta.__call__ checks this at instantiation time.
_MinimalHandler.__abstractmethods__ = frozenset()


class TestRunCommandTimeout:
    @pytest.mark.asyncio
    async def test_no_timeout_defaults_to_indefinite_wait(self, monkeypatch):
        # Ensure we hit the subprocess_shell branch (not the android adb branch).
        monkeypatch.delenv("IS_CUA_ANDROID", raising=False)
        h = _MinimalHandler()

        result = await h.run_command("echo hello")
        assert result["success"] is True
        assert result["stdout"].strip() == "hello"
        assert result["return_code"] == 0

    @pytest.mark.asyncio
    async def test_timeout_honoured_when_passed(self, monkeypatch):
        monkeypatch.delenv("IS_CUA_ANDROID", raising=False)
        h = _MinimalHandler()

        # sleep 5, cap at 0.2 — should time out
        start = asyncio.get_event_loop().time()
        result = await h.run_command("sleep 5", timeout=0.2)
        elapsed = asyncio.get_event_loop().time() - start

        assert result["success"] is False
        assert "timed out" in result["stderr"].lower()
        assert result["return_code"] == -1
        # Must have returned within a small factor of the requested timeout.
        assert elapsed < 2.0, f"expected quick timeout, took {elapsed:.2f}s"

    @pytest.mark.asyncio
    async def test_fast_command_under_timeout_succeeds(self, monkeypatch):
        monkeypatch.delenv("IS_CUA_ANDROID", raising=False)
        h = _MinimalHandler()

        result = await h.run_command("echo done", timeout=10.0)
        assert result["success"] is True
        assert result["stdout"].strip() == "done"

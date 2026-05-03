"""Unit tests for VM cleanup on connection failure and destroy() resilience.

These tests mock CloudTransport so they run without a real cloud API.
They verify that:
  1. _create() cleans up a provisioned VM when _connect() fails.
  2. destroy() runs every cleanup step independently — a failure in one
     does not prevent the others from executing.
"""

from __future__ import annotations

from unittest.mock import AsyncMock, MagicMock, patch

import httpx
import pytest
from cua_sandbox.image import Image
from cua_sandbox.sandbox import Sandbox
from cua_sandbox.transport.cloud import CloudTransport

pytestmark = pytest.mark.asyncio


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _make_cloud_transport(*, name: str = "test-vm") -> CloudTransport:
    """Return a CloudTransport with internal state set as if _create_vm() succeeded."""
    t = CloudTransport.__new__(CloudTransport)
    t._name = name
    t._api_key_override = "sk-fake"
    t._base_url = "https://api.example.com"
    t._image = None
    t._cpu = None
    t._memory_mb = None
    t._disk_gb = None
    t._region = "us-east-1"
    t._inner = None
    t._api_client = None
    return t


def _make_sandbox(transport: CloudTransport, **kwargs) -> Sandbox:
    """Return a Sandbox wrapping *transport* without calling _connect()."""
    return Sandbox(
        transport,
        name=transport._name,
        _ephemeral=kwargs.get("ephemeral", True),
        _telemetry_enabled=False,
    )


# ===================================================================
# 1. _create() cleans up on _connect() failure
# ===================================================================


class TestCreateCleansUpOnConnectFailure:
    """Sandbox._create() should delete the cloud VM when _connect() raises."""

    async def test_delete_vm_called_on_timeout(self):
        """ReadTimeout during _connect() triggers delete_vm()."""
        transport = _make_cloud_transport(name="orphan-vm")
        transport.connect = AsyncMock(side_effect=httpx.ReadTimeout("poll timed out"))
        transport.delete_vm = AsyncMock()

        with patch(
            "cua_sandbox.sandbox.CloudTransport",
            return_value=transport,
        ):
            with pytest.raises(httpx.ReadTimeout):
                await Sandbox._create(
                    image=Image.linux("ubuntu", "24.04"),
                    api_key="sk-fake",
                    telemetry_enabled=False,
                )

        transport.delete_vm.assert_awaited_once()

    async def test_delete_vm_called_on_generic_exception(self):
        """Any exception during _connect() triggers delete_vm()."""
        transport = _make_cloud_transport(name="orphan-vm")
        transport.connect = AsyncMock(side_effect=RuntimeError("unexpected"))
        transport.delete_vm = AsyncMock()

        with patch(
            "cua_sandbox.sandbox.CloudTransport",
            return_value=transport,
        ):
            with pytest.raises(RuntimeError, match="unexpected"):
                await Sandbox._create(
                    image=Image.linux("ubuntu", "24.04"),
                    api_key="sk-fake",
                    telemetry_enabled=False,
                )

        transport.delete_vm.assert_awaited_once()

    async def test_original_exception_propagates_even_if_delete_fails(self):
        """The original connect error is re-raised even when delete_vm also fails."""
        transport = _make_cloud_transport(name="orphan-vm")
        transport.connect = AsyncMock(side_effect=TimeoutError("poll timeout"))
        transport.delete_vm = AsyncMock(side_effect=httpx.ConnectError("api down"))

        with patch(
            "cua_sandbox.sandbox.CloudTransport",
            return_value=transport,
        ):
            with pytest.raises(TimeoutError, match="poll timeout"):
                await Sandbox._create(
                    image=Image.linux("ubuntu", "24.04"),
                    api_key="sk-fake",
                    telemetry_enabled=False,
                )

        transport.delete_vm.assert_awaited_once()

    async def test_no_cleanup_when_vm_not_yet_created(self):
        """If connect() fails before _create_vm (no _name), skip delete_vm."""
        transport = _make_cloud_transport()
        # Simulate: connect() fails before _create_vm sets _name
        transport._name = None
        transport.connect = AsyncMock(side_effect=ValueError("no api key"))
        transport.delete_vm = AsyncMock()

        with patch(
            "cua_sandbox.sandbox.CloudTransport",
            return_value=transport,
        ):
            with pytest.raises(ValueError, match="no api key"):
                await Sandbox._create(
                    image=Image.linux("ubuntu", "24.04"),
                    api_key="sk-fake",
                    telemetry_enabled=False,
                )

        transport.delete_vm.assert_not_awaited()

    async def test_keyboard_interrupt_still_cleans_up(self):
        """BaseException subclasses (KeyboardInterrupt) also trigger cleanup."""
        transport = _make_cloud_transport(name="interrupted-vm")
        transport.connect = AsyncMock(side_effect=KeyboardInterrupt)
        transport.delete_vm = AsyncMock()

        with patch(
            "cua_sandbox.sandbox.CloudTransport",
            return_value=transport,
        ):
            with pytest.raises(KeyboardInterrupt):
                await Sandbox._create(
                    image=Image.linux("ubuntu", "24.04"),
                    api_key="sk-fake",
                    telemetry_enabled=False,
                )

        transport.delete_vm.assert_awaited_once()


# ===================================================================
# 2. destroy() is resilient to individual step failures
# ===================================================================


class TestDestroyResilience:
    """Each cleanup step in destroy() should run independently."""

    async def test_delete_vm_runs_even_if_disconnect_fails(self):
        """A failing disconnect() must not prevent delete_vm()."""
        transport = _make_cloud_transport(name="leaky-vm")
        transport.disconnect = AsyncMock(side_effect=OSError("connection reset"))
        transport.delete_vm = AsyncMock()

        sb = _make_sandbox(transport)
        await sb.destroy()

        transport.disconnect.assert_awaited_once()
        transport.delete_vm.assert_awaited_once()

    async def test_runtime_stop_runs_even_if_delete_vm_fails(self):
        """A failing delete_vm() must not prevent runtime cleanup."""
        transport = _make_cloud_transport(name="leaky-vm")
        transport.disconnect = AsyncMock()
        transport.delete_vm = AsyncMock(side_effect=httpx.ConnectError("api down"))

        runtime = AsyncMock()
        runtime_info = MagicMock()
        runtime_info.name = "leaky-vm"

        sb = _make_sandbox(transport)
        sb._runtime = runtime
        sb._runtime_info = runtime_info

        await sb.destroy()

        transport.delete_vm.assert_awaited_once()
        runtime.delete.assert_awaited_once_with("leaky-vm")

    async def test_destroy_succeeds_when_all_steps_fail(self):
        """destroy() must not raise even if every cleanup step fails."""
        transport = _make_cloud_transport(name="total-fail")
        transport.disconnect = AsyncMock(side_effect=OSError("disconnect fail"))
        transport.delete_vm = AsyncMock(side_effect=httpx.ReadTimeout("delete fail"))

        runtime = AsyncMock()
        runtime.delete = AsyncMock(side_effect=RuntimeError("runtime fail"))
        runtime_info = MagicMock()
        runtime_info.name = "total-fail"

        sb = _make_sandbox(transport)
        sb._runtime = runtime
        sb._runtime_info = runtime_info

        # Should not raise
        await sb.destroy()

        transport.disconnect.assert_awaited_once()
        transport.delete_vm.assert_awaited_once()
        runtime.delete.assert_awaited_once()

    async def test_destroy_happy_path(self):
        """All steps succeed — basic smoke test."""
        transport = _make_cloud_transport(name="good-vm")
        transport.disconnect = AsyncMock()
        transport.delete_vm = AsyncMock()

        sb = _make_sandbox(transport)
        await sb.destroy()

        transport.disconnect.assert_awaited_once()
        transport.delete_vm.assert_awaited_once()

    async def test_non_cloud_transport_skips_delete_vm(self):
        """Non-CloudTransport sandboxes should not call delete_vm."""
        transport = AsyncMock()  # generic mock, not a CloudTransport instance
        sb = Sandbox(transport, name="local-vm", _ephemeral=True, _telemetry_enabled=False)

        await sb.destroy()

        transport.disconnect.assert_awaited_once()
        # delete_vm should not be called since transport is not CloudTransport
        assert not hasattr(transport, "delete_vm") or not transport.delete_vm.called


# ===================================================================
# 3. ephemeral() integration — cleanup through the context manager
# ===================================================================


class TestEphemeralCleanup:
    """Sandbox.ephemeral() should destroy the VM on normal and error exits."""

    async def test_ephemeral_destroys_on_normal_exit(self):
        """VM is destroyed when the async-with block exits normally."""
        transport = _make_cloud_transport(name="eph-ok")
        transport.connect = AsyncMock()
        transport.disconnect = AsyncMock()
        transport.delete_vm = AsyncMock()

        with (
            patch.object(
                CloudTransport,
                "__init__",
                lambda self, **kw: None,
            ),
            patch.object(
                CloudTransport,
                "__new__",
                lambda cls, **kw: transport,
            ),
        ):
            async with Sandbox.ephemeral(
                Image.linux("ubuntu", "24.04"),
                api_key="sk-fake",
                telemetry_enabled=False,
            ) as sb:
                assert sb.name == "eph-ok"

        transport.delete_vm.assert_awaited_once()

    async def test_ephemeral_destroys_on_test_failure(self):
        """VM is destroyed even when the body raises an assertion error."""
        transport = _make_cloud_transport(name="eph-fail")
        transport.connect = AsyncMock()
        transport.disconnect = AsyncMock()
        transport.delete_vm = AsyncMock()

        with (
            patch.object(
                CloudTransport,
                "__init__",
                lambda self, **kw: None,
            ),
            patch.object(
                CloudTransport,
                "__new__",
                lambda cls, **kw: transport,
            ),
        ):
            with pytest.raises(AssertionError):
                async with Sandbox.ephemeral(
                    Image.linux("ubuntu", "24.04"),
                    api_key="sk-fake",
                    telemetry_enabled=False,
                ) as _sb:
                    raise AssertionError("test failed")

        transport.delete_vm.assert_awaited_once()

    async def test_ephemeral_cleans_up_when_create_connect_fails(self):
        """If _create raises after VM provisioning, the VM is still cleaned up."""
        transport = _make_cloud_transport(name="eph-connect-fail")
        transport.connect = AsyncMock(side_effect=httpx.ReadTimeout("poll timed out"))
        transport.delete_vm = AsyncMock()

        with (
            patch.object(
                CloudTransport,
                "__init__",
                lambda self, **kw: None,
            ),
            patch.object(
                CloudTransport,
                "__new__",
                lambda cls, **kw: transport,
            ),
        ):
            with pytest.raises(httpx.ReadTimeout):
                async with Sandbox.ephemeral(
                    Image.linux("ubuntu", "24.04"),
                    api_key="sk-fake",
                    telemetry_enabled=False,
                ) as _sb:
                    pass  # never reached

        transport.delete_vm.assert_awaited_once()

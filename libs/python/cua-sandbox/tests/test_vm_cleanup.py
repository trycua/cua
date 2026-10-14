"""Unit tests for VM cleanup on connection failure and destroy() resilience.

These tests mock FleetCloudTransport so they run without a real cloud API.
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
from cua_sandbox.transport.fleet_cloud import FleetCloudTransport

pytestmark = pytest.mark.asyncio


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _make_cloud_transport(*, name: str = "test-vm") -> FleetCloudTransport:
    """Return a FleetCloudTransport with internal state set as if _create_vm() succeeded."""
    t = FleetCloudTransport.__new__(FleetCloudTransport)
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


def _managed_sandbox() -> MagicMock:
    """A sandbox as returned by cua_sandbox._autopool.acquire."""
    claimed = MagicMock()
    claimed.close = AsyncMock()
    claimed.keep_alive = AsyncMock()
    return claimed


def _make_sandbox(transport: FleetCloudTransport, **kwargs) -> Sandbox:
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
    """Sandbox._create() releases a managed Fleet claim when attaching fails.

    Fleet images come from the account's managed pool (the cua SDK's native
    manager); these run against the ``cua-test-fixtures`` fake Fleet API.
    """

    @pytest.fixture(autouse=True)
    def _fleet(self, fleet):
        self.client_factory = fleet

    async def _claims(self) -> list[str]:
        client = self.client_factory()
        try:
            names = [n.name for n in await client.list_namespaces()]
            out: list[str] = []
            for ns in names:
                out += [c.metadata.name for c in await client.list_claims(ns)]
            return out
        finally:
            await client.close()

    def _fail_attach(self, monkeypatch, error: BaseException) -> None:
        async def wait(self, **_):
            raise error

        monkeypatch.setattr("cua_sandbox.pool._ClaimHandle.wait", wait)

    async def _create(self):
        return await Sandbox._create(
            image=Image.from_registry("registry.example/workspace@sha256:0123"),
            telemetry_enabled=False,
        )

    async def test_delete_vm_called_on_timeout(self, monkeypatch):
        """ReadTimeout while attaching releases the claim and keeps its type."""
        self._fail_attach(monkeypatch, httpx.ReadTimeout("poll timed out"))
        with pytest.raises(httpx.ReadTimeout):
            await self._create()
        assert not await self._claims()

    async def test_delete_vm_called_on_generic_exception(self, monkeypatch):
        self._fail_attach(monkeypatch, RuntimeError("unexpected"))
        with pytest.raises(RuntimeError, match="unexpected"):
            await self._create()
        assert not await self._claims()

    async def test_original_exception_propagates_even_if_delete_fails(self, monkeypatch):
        from cua_sandbox import _autopool

        self._fail_attach(monkeypatch, TimeoutError("poll timeout"))

        async def release(self):
            raise httpx.ConnectError("api down")

        monkeypatch.setattr(_autopool.ManagedClaimHandle, "release", release)
        with pytest.raises(TimeoutError, match="poll timeout"):
            await self._create()

    async def test_no_cleanup_when_vm_not_yet_created(self, monkeypatch):
        """A failure before any claim exists deletes nothing."""
        from cua_sandbox import _autopool

        def no_runtime():
            raise ValueError("no api key")

        monkeypatch.setattr(_autopool, "_native_cua", no_runtime)
        with pytest.raises(_autopool.AutoPoolError, match="no api key"):
            await self._create()
        assert not await self._claims()

    async def test_keyboard_interrupt_still_cleans_up(self, monkeypatch):
        self._fail_attach(monkeypatch, KeyboardInterrupt())
        with pytest.raises(KeyboardInterrupt):
            await self._create()
        assert not await self._claims()


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
        """Non-FleetCloudTransport sandboxes should not call delete_vm."""
        transport = AsyncMock()  # generic mock, not a FleetCloudTransport instance
        sb = Sandbox(transport, name="local-vm", _ephemeral=True, _telemetry_enabled=False)

        await sb.destroy()

        transport.disconnect.assert_awaited_once()
        # delete_vm should not be called since transport is not FleetCloudTransport
        assert not hasattr(transport, "delete_vm") or not transport.delete_vm.called


# ===================================================================
# 3. Fleet server_port forwarding and validation
# ===================================================================


class TestFleetServerPortForwarding:
    """Sandbox factories should pass server_port to Fleet and validate it early."""

    @pytest.fixture(autouse=True)
    def _select_fleet(self, monkeypatch):
        monkeypatch.setattr(Sandbox, "_uses_fleet", staticmethod(lambda api_key: api_key is None))

    async def test_create_forwards_server_port_to_the_managed_pool(self):
        # Was test_create_with_fleet_image_requires_explicit_pool.
        apply = AsyncMock()
        acquire = AsyncMock(return_value=_managed_sandbox())

        with (
            patch("cua_sandbox.pool.Pool.apply", new=apply),
            patch("cua_sandbox._autopool.acquire", new=acquire),
            patch("cua_sandbox.sandbox._save_fleet_claim_or_close", new=AsyncMock()),
        ):
            await Sandbox.create(
                Image.from_registry("registry.example/workspace:latest"),
                server_port=5000,
                telemetry_enabled=False,
                local=False,
            )

        apply.assert_not_awaited()
        assert acquire.await_args.kwargs["server_port"] == 5000

    async def test_ephemeral_forwards_server_port(self):
        claimed = _managed_sandbox()
        acquire = AsyncMock(return_value=claimed)

        with patch("cua_sandbox._autopool.acquire", new=acquire):
            async with Sandbox.ephemeral(
                Image.from_registry("registry.example/workspace:latest"),
                server_port=5000,
                telemetry_enabled=False,
                local=False,
            ):
                pass

        assert acquire.await_args.kwargs["server_port"] == 5000
        claimed.close.assert_awaited_once()

    async def test_create_passes_server_port_to_fleet_transport(self):
        # _create (the sandbox() helper path) routes to the managed pool too.
        acquire = AsyncMock(return_value=_managed_sandbox())

        with (
            patch.object(Sandbox, "_uses_fleet", return_value=True),
            patch("cua_sandbox._autopool.acquire", new=acquire),
            patch("cua_sandbox.sandbox._save_fleet_claim_or_close", new=AsyncMock()),
        ):
            await Sandbox._create(
                image=Image.from_registry("registry.example/workspace:latest"),
                server_port=5000,
                telemetry_enabled=False,
            )

        assert acquire.await_args.kwargs["server_port"] == 5000

    async def test_existing_pool_does_not_pass_server_port_to_fleet_transport(self):
        transport = _make_cloud_transport(name="existing-pool")

        with (
            patch.object(Sandbox, "_uses_fleet", return_value=True),
            patch(
                "cua_sandbox.sandbox.FleetCloudTransport",
                return_value=transport,
            ) as fleet_transport,
            patch.object(Sandbox, "_connect", AsyncMock()),
        ):
            await Sandbox._create(
                name="existing-pool",
                server_port=5000,
                telemetry_enabled=False,
            )

        assert "server_port" not in fleet_transport.call_args.kwargs

    @pytest.mark.parametrize("server_port", [True, False, 0, -1, 65536, 5000.0, "5000"])
    async def test_create_rejects_invalid_server_port_before_local_provisioning(self, server_port):
        runtime = AsyncMock()

        with pytest.raises(ValueError, match="server_port must be an integer between 1 and 65535"):
            await Sandbox.create(
                Image.from_registry("registry.example/workspace:latest"),
                local=True,
                runtime=runtime,
                server_port=server_port,
                telemetry_enabled=False,
            )

        runtime.start.assert_not_awaited()

    @pytest.mark.parametrize("server_port", [True, False, 0, -1, 65536, 5000.0, "5000"])
    async def test_invalid_server_port_rejects_before_legacy_cloud_provisioning(self, server_port):
        with patch("cua_sandbox.sandbox._make_transport") as make_transport:
            with pytest.raises(
                ValueError, match="server_port must be an integer between 1 and 65535"
            ):
                await Sandbox._create(
                    image=Image.from_registry("registry.example/workspace:latest"),
                    api_key="sk-legacy",
                    server_port=server_port,
                    telemetry_enabled=False,
                )

        make_transport.assert_not_called()

    @pytest.mark.parametrize("server_port", [True, False, 0, -1, 65536, 5000.0, "5000"])
    async def test_invalid_server_port_rejects_before_fleet_provisioning(self, server_port):
        with patch("cua_sandbox.sandbox.FleetCloudTransport") as fleet_transport:
            with pytest.raises(
                ValueError, match="server_port must be an integer between 1 and 65535"
            ):
                await Sandbox._create(
                    image=Image.from_registry("registry.example/workspace:latest"),
                    server_port=server_port,
                    telemetry_enabled=False,
                )

        fleet_transport.assert_not_called()


# ===================================================================
# 4. ephemeral() integration — cleanup through the context manager
# ===================================================================


class TestEphemeralCleanup:
    """Sandbox.ephemeral() closes Fleet claims on every exit path."""

    @pytest.fixture(autouse=True)
    def _select_fleet(self, monkeypatch):
        monkeypatch.setattr(Sandbox, "_uses_fleet", staticmethod(lambda api_key: api_key is None))

    async def test_ephemeral_destroys_on_normal_exit(self):
        claimed = _managed_sandbox()
        with patch("cua_sandbox._autopool.acquire", new=AsyncMock(return_value=claimed)):
            async with Sandbox.ephemeral(
                Image.from_registry("registry.example/workspace:latest"),
                telemetry_enabled=False,
                local=False,
            ):
                pass

        claimed.close.assert_awaited_once()

    async def test_ephemeral_destroys_on_test_failure(self):
        claimed = _managed_sandbox()
        with patch("cua_sandbox._autopool.acquire", new=AsyncMock(return_value=claimed)):
            with pytest.raises(AssertionError):
                async with Sandbox.ephemeral(
                    Image.from_registry("registry.example/workspace:latest"),
                    telemetry_enabled=False,
                    local=False,
                ):
                    raise AssertionError("test failed")

        claimed.close.assert_awaited_once()

    async def test_ephemeral_propagates_claim_failure(self):
        acquire = AsyncMock(side_effect=httpx.ReadTimeout("poll timed out"))
        with patch("cua_sandbox._autopool.acquire", new=acquire):
            with pytest.raises(httpx.ReadTimeout):
                async with Sandbox.ephemeral(
                    Image.from_registry("registry.example/workspace:latest"),
                    telemetry_enabled=False,
                    local=False,
                ):
                    pass

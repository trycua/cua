"""Unit tests for ``destroy()`` resilience and ``ephemeral()`` cleanup.

They verify that:
  1. destroy() runs every cleanup step independently: a failure in one does
     not prevent the others from executing.
  2. server_port is validated before anything is provisioned.
  3. ephemeral() destroys the sandbox on every exit path.
"""

from __future__ import annotations

from unittest.mock import AsyncMock, MagicMock, patch

import pytest
from cua_sandbox.image import Image
from cua_sandbox.sandbox import Sandbox

pytestmark = pytest.mark.asyncio


def _make_sandbox(*, name: str = "test-vm", ephemeral: bool = True) -> Sandbox:
    """A sandbox over a mock transport, without calling _connect()."""
    return Sandbox(AsyncMock(), name=name, _ephemeral=ephemeral, _telemetry_enabled=False)


def _with_runtime(sb: Sandbox, *, name: str, delete: AsyncMock | None = None) -> AsyncMock:
    runtime = AsyncMock()
    if delete is not None:
        runtime.delete = delete
    runtime_info = MagicMock()
    runtime_info.name = name
    sb._runtime = runtime
    sb._runtime_info = runtime_info
    return runtime


# ===================================================================
# 1. destroy() resilience
# ===================================================================


class TestDestroyResilience:
    """Each cleanup step in destroy() should run independently."""

    async def test_runtime_delete_runs_even_if_disconnect_fails(self):
        """A failing disconnect() must not prevent the runtime cleanup."""
        sb = _make_sandbox(name="leaky-vm")
        sb._transport.disconnect = AsyncMock(side_effect=OSError("connection reset"))
        runtime = _with_runtime(sb, name="leaky-vm")

        await sb.destroy()

        sb._transport.disconnect.assert_awaited_once()
        runtime.delete.assert_awaited_once_with("leaky-vm")

    async def test_destroy_succeeds_when_all_steps_fail(self):
        """destroy() must not raise even if every cleanup step fails."""
        sb = _make_sandbox(name="total-fail")
        sb._transport.disconnect = AsyncMock(side_effect=OSError("disconnect fail"))
        runtime = _with_runtime(
            sb, name="total-fail", delete=AsyncMock(side_effect=RuntimeError("runtime fail"))
        )

        # Should not raise
        await sb.destroy()

        sb._transport.disconnect.assert_awaited_once()
        runtime.delete.assert_awaited_once()

    async def test_destroy_happy_path(self):
        """All steps succeed: basic smoke test."""
        sb = _make_sandbox(name="good-vm")
        runtime = _with_runtime(sb, name="good-vm")

        await sb.destroy()

        sb._transport.disconnect.assert_awaited_once()
        runtime.delete.assert_awaited_once_with("good-vm")

    async def test_a_persistent_sandbox_is_stopped_not_deleted(self):
        sb = _make_sandbox(name="kept-vm", ephemeral=False)
        runtime = _with_runtime(sb, name="kept-vm")

        await sb.destroy()

        runtime.stop.assert_awaited_once_with("kept-vm")
        runtime.delete.assert_not_awaited()

    async def test_a_sandbox_without_a_runtime_only_disconnects(self):
        sb = _make_sandbox(name="direct-vm")

        await sb.destroy()

        sb._transport.disconnect.assert_awaited_once()


# ===================================================================
# 2. server_port validation
# ===================================================================


class TestServerPortValidation:
    """Sandbox factories validate server_port before provisioning anything."""

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
    async def test_internal_create_rejects_invalid_server_port(self, server_port):
        runtime = AsyncMock()

        with pytest.raises(ValueError, match="server_port must be an integer between 1 and 65535"):
            await Sandbox._create(
                image=Image.from_registry("registry.example/workspace:latest"),
                local=True,
                runtime=runtime,
                server_port=server_port,
                telemetry_enabled=False,
            )

        runtime.start.assert_not_awaited()


# ===================================================================
# 3. ephemeral() integration: cleanup through the context manager
# ===================================================================


class TestEphemeralCleanup:
    """Sandbox.ephemeral() destroys the sandbox on every exit path."""

    @staticmethod
    def _created():
        created = MagicMock()
        created.destroy = AsyncMock()
        return created

    async def test_ephemeral_destroys_on_normal_exit(self):
        created = self._created()
        with patch.object(Sandbox, "_create", new=AsyncMock(return_value=created)):
            async with Sandbox.ephemeral(
                Image.from_registry("registry.example/workspace:latest"),
                local=True,
                telemetry_enabled=False,
            ) as sb:
                assert sb is created

        created.destroy.assert_awaited_once()

    async def test_ephemeral_destroys_on_test_failure(self):
        created = self._created()
        with patch.object(Sandbox, "_create", new=AsyncMock(return_value=created)):
            with pytest.raises(AssertionError):
                async with Sandbox.ephemeral(
                    Image.from_registry("registry.example/workspace:latest"),
                    local=True,
                    telemetry_enabled=False,
                ):
                    raise AssertionError("test failed")

        created.destroy.assert_awaited_once()

    async def test_ephemeral_propagates_create_failure(self):
        create = AsyncMock(side_effect=TimeoutError("start timed out"))
        with patch.object(Sandbox, "_create", new=create):
            with pytest.raises(TimeoutError):
                async with Sandbox.ephemeral(
                    Image.from_registry("registry.example/workspace:latest"),
                    local=True,
                    telemetry_enabled=False,
                ):
                    pass

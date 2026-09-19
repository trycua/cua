"""Tunnel acquisition rollback and normal cleanup."""

import asyncio
from unittest.mock import AsyncMock, call

import pytest
from cua_sandbox.interfaces.tunnel import Tunnel, TunnelInfo
from cua_sandbox.transport.base import Transport

pytestmark = pytest.mark.asyncio


@pytest.mark.parametrize("use_context", [False, True])
async def test_partial_acquisition_closes_opened_tunnel(use_context):
    transport = AsyncMock(spec=Transport)
    info = TunnelInfo("localhost", 45001, 9222)
    error = RuntimeError("second forward failed")
    transport.forward_tunnel.side_effect = [info, error]
    context = Tunnel(transport).forward(9222, 8080)

    with pytest.raises(RuntimeError) as caught:
        if use_context:
            async with context:
                pytest.fail("context body must not run after failed acquisition")
        else:
            await context

    assert caught.value is error
    transport.close_tunnel.assert_awaited_once_with(info)
    await info.close()
    transport.close_tunnel.assert_awaited_once_with(info)


async def test_cancelled_acquisition_closes_opened_tunnel():
    transport = AsyncMock(spec=Transport)
    info = TunnelInfo("localhost", 45001, 9222)
    second_started = asyncio.Event()

    async def forward(port):
        if port == 9222:
            return info
        second_started.set()
        await asyncio.Future()

    transport.forward_tunnel.side_effect = forward
    task = asyncio.ensure_future(Tunnel(transport).forward(9222, 8080))
    await second_started.wait()
    task.cancel()

    with pytest.raises(asyncio.CancelledError):
        await task

    transport.close_tunnel.assert_awaited_once_with(info)


@pytest.mark.parametrize("close_error", [RuntimeError("close failed"), asyncio.CancelledError()])
async def test_rollback_attempts_all_closers_and_preserves_original_error(close_error):
    transport = AsyncMock(spec=Transport)
    first = TunnelInfo("localhost", 45001, 9222)
    second = TunnelInfo("localhost", 45002, 8080)
    error = RuntimeError("third forward failed")
    transport.forward_tunnel.side_effect = [first, second, error]
    transport.close_tunnel.side_effect = [close_error, None]

    with pytest.raises(RuntimeError) as caught:
        await Tunnel(transport).forward(9222, 8080, 3000)

    assert caught.value is error
    assert transport.close_tunnel.await_args_list == [call(first), call(second)]
    transport.close_tunnel.side_effect = None
    await first.close()
    await second.close()
    assert transport.close_tunnel.await_args_list == [call(first), call(second), call(first)]


async def test_failed_attempt_preserves_previously_opened_tunnels():
    transport = AsyncMock(spec=Transport)
    first = TunnelInfo("localhost", 45001, 9222)
    second = TunnelInfo("localhost", 45002, 8080)
    third = TunnelInfo("localhost", 45003, 9222)
    transport.forward_tunnel.side_effect = [first, second, third, RuntimeError("forward failed")]
    context = Tunnel(transport).forward(9222, 8080)
    assert await context == {9222: first, 8080: second}

    with pytest.raises(RuntimeError, match="forward failed"):
        await context

    transport.close_tunnel.assert_awaited_once_with(third)
    await first.close()
    await second.close()
    assert transport.close_tunnel.await_args_list == [call(third), call(first), call(second)]


async def test_successful_context_closes_all_tunnels():
    transport = AsyncMock(spec=Transport)
    first = TunnelInfo("localhost", 45001, 9222)
    second = TunnelInfo("localhost", 45002, 8080)
    transport.forward_tunnel.side_effect = [first, second]

    async with Tunnel(transport).forward(9222, 8080) as tunnels:
        assert tunnels == {9222: first, 8080: second}
        transport.close_tunnel.assert_not_awaited()

    assert transport.close_tunnel.await_args_list == [call(first), call(second)]


async def test_successful_await_leaves_tunnel_open_until_closed():
    transport = AsyncMock(spec=Transport)
    info = TunnelInfo("localhost", 45001, 9222)
    transport.forward_tunnel.return_value = info

    assert await Tunnel(transport).forward(9222) is info
    transport.close_tunnel.assert_not_awaited()
    await info.close()
    await info.close()
    transport.close_tunnel.assert_awaited_once_with(info)

"""F61: Relay.* calls work from asyncio and from plain sync code (no Tokio
reactor on the caller's side); a closed relay is an error, not a panic."""

import asyncio

import pytest

import cua


def _relay():
    return cua.Relay("http://127.0.0.1:9", "token")


async def _calls():
    r = _relay()
    for call in (r.machines(), r.machine("m1"), r.stop_sharing("m1"), r.start_sharing("m1"), r.connect("m1")):
        with pytest.raises(cua.CuaError):
            await call


def test_relay_calls_fail_cleanly_under_asyncio():
    asyncio.run(_calls())


def test_relay_calls_fail_cleanly_in_a_plain_thread_loop():
    loop = asyncio.new_event_loop()
    try:
        loop.run_until_complete(_calls())
    finally:
        loop.close()

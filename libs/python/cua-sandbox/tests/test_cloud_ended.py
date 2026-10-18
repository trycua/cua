"""Cua Cloud has ended (CUA-1854): every cloud entry point raises one message."""

from __future__ import annotations

import pytest
from cua_sandbox import Image, Pool, Sandbox, pools
from cua_sandbox._ended import CLOUD_ENDED, CloudEnded

MESSAGE = (
    "Cua Cloud has ended; use a local sandbox or your own machine. "
    "See https://cua.ai/docs/cua-sdk/guides/cua-cloud-ended"
)


def test_the_message_is_exact():
    assert CLOUD_ENDED == MESSAGE
    assert str(CloudEnded()) == MESSAGE
    assert isinstance(CloudEnded(), NotImplementedError)


@pytest.mark.parametrize("kwargs", [{"on": "cloud"}, {"local": False}])
async def test_create_and_ephemeral_fail_fast(kwargs):
    with pytest.raises(CloudEnded, match="Cua Cloud has ended"):
        await Sandbox.create(Image.linux(), **kwargs)
    with pytest.raises(CloudEnded, match="Cua Cloud has ended"):
        async with Sandbox.ephemeral(Image.linux(), **kwargs):
            pass


async def test_the_default_location_set_to_cloud_fails(monkeypatch):
    monkeypatch.setenv("CUA_DEFAULT_ON", "cloud")
    with pytest.raises(CloudEnded):
        await Sandbox.create(Image.linux())


async def test_references_to_cloud_sandboxes_fail():
    for call in (
        lambda: Sandbox.connect("cloud:box"),
        lambda: Sandbox.connect("box", local=False),
        lambda: Sandbox.get_info("cloud:box"),
        lambda: Sandbox.suspend("cloud:box"),
        lambda: Sandbox.resume("cloud:box"),
        lambda: Sandbox.delete("cloud:box"),
        lambda: Sandbox.list(local=False),
        lambda: Sandbox.list(location="cloud"),
    ):
        with pytest.raises(CloudEnded) as error:
            await call()
        assert str(error.value) == MESSAGE


async def test_pools_fail():
    with pytest.raises(CloudEnded):
        await Pool.get("p")
    with pytest.raises(CloudEnded):
        await pools.list_pools()
    with pytest.raises(CloudEnded):
        await pools.gc()


async def test_the_default_listing_is_local_only(monkeypatch):
    async def local():
        return []

    monkeypatch.setattr(Sandbox, "_list_local", classmethod(lambda cls: local()))
    assert await Sandbox.list() == []

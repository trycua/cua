"""Cua Cloud has ended: every cloud entry point raises one message."""

from __future__ import annotations

import importlib

import pytest
from cua_sandbox import Image, Sandbox
from cua_sandbox._ended import CLOUD_ENDED, CloudEnded
from cua_sandbox.runtime.base import RuntimeInfo

sandbox_module = importlib.import_module("cua_sandbox.sandbox")

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


async def test_the_default_listing_is_local_only(monkeypatch):
    async def local():
        return []

    monkeypatch.setattr(Sandbox, "_list_local", classmethod(lambda cls: local()))
    assert await Sandbox.list() == []


async def test_local_creation_with_a_runtime_object_writes_its_state_file(monkeypatch, tmp_path):
    # The named local sandbox writes a state file; keep it in this test's
    # own CUA_HOME (the autouse conftest one too, but never rely on it).
    monkeypatch.setenv("CUA_HOME", str(tmp_path))
    calls = []

    class Runtime:
        # Sandbox._create tells the runtime whether the sandbox is ephemeral;
        # a double that omits it fails the call rather than the assertion.
        async def start(self, image, name, *, ephemeral=True):
            calls.append(("start", image, name))
            return RuntimeInfo(host="127.0.0.1", api_port=8000, name=name, environment="linux")

    sandbox = await Sandbox.create(Image.linux(), name="local-demo", local=True, runtime=Runtime())
    await sandbox.disconnect()

    assert calls[0][2] == "local-demo"
    assert (tmp_path / "sandboxes" / "local-demo.json").exists()


async def test_a_pool_name_selects_the_cloud_and_fails_without_a_warning():
    import warnings

    with warnings.catch_warnings():
        warnings.simplefilter("error")
        with pytest.raises(CloudEnded):
            await Sandbox.create(Image.linux(), pool="my-pool")


async def test_a_pool_name_fails_even_when_local_is_explicit():
    with pytest.raises(CloudEnded):
        await Sandbox.create(Image.linux(), local=True, pool="my-pool")
    with pytest.raises(CloudEnded):
        async with Sandbox.ephemeral(Image.linux(), local=True, pool="my-pool"):
            pass


async def test_an_unknown_bare_name_is_not_found_locally(monkeypatch):
    # A bare name used to fall through to the cloud; now it is a local lookup.
    monkeypatch.setattr(sandbox_module, "_remove_orphan_container", lambda name: False)
    with pytest.raises(ValueError, match="No local sandbox named 'nope'"):
        await Sandbox.connect("nope")
    with pytest.raises(ValueError, match="No local sandbox named 'nope'"):
        await Sandbox.delete("nope")

"""The meta-package surface of `cua` without the optional extras installed."""

from __future__ import annotations

import importlib.util

import pytest

import cua


def test_native_handle_is_always_available():
    assert cua.SandboxHandle is cua._native.Sandbox


@pytest.mark.skipif(
    importlib.util.find_spec("cua_sandbox") is not None, reason="cua-sandbox is installed"
)
def test_sandbox_falls_back_to_the_native_handle_without_the_extra():
    assert cua.Sandbox is cua.SandboxHandle
    # Without the extra, Image is the SDK's canonical-reference helper.
    from cua.images import Image

    assert cua.Image is Image


@pytest.mark.skipif(
    importlib.util.find_spec("cua_agent") is not None, reason="cua-agent is installed"
)
def test_agent_names_point_at_the_agent_extra():
    with pytest.raises(ImportError, match=r"cua\[agent\]"):
        cua.ComputerAgent  # noqa: B018


def test_unknown_names_still_raise_attribute_error():
    with pytest.raises(AttributeError):
        cua.definitely_not_a_name  # noqa: B018


def test_env_driver_era_names_are_gone():
    # The env-driver names never shipped in a release; there are no aliases.
    for name in ("EnvClient", "EnvProcess", "EnvDriverNotAvailable"):
        assert not hasattr(cua, name), name
    assert not hasattr(cua.CuaError, "EnvDriverNotAvailable")
    assert cua.SpaceCreateOptions(spacesd=False).spacesd is False
    with pytest.raises(TypeError):
        cua.SpaceCreateOptions(env_driver=False)


def test_sandbox_refs_parse_qualify_and_carry_candidates():
    # One ref scheme: qualified ids round-trip, legacy spellings parse.
    for ref in ("local:box", "cloud:box", "direct:10.0.0.5:3211", "relay:0123abcd4567ef89"):
        assert cua.parse_sandbox_ref(ref).id == ref
    legacy = cua.parse_sandbox_ref("space://fleet/ns/box")
    assert (legacy.location, legacy.name, legacy.id) == ("cloud", "box", "cloud:box")
    assert cua.parse_sandbox_ref("fleet:ns:box").id == "cloud:box"
    assert cua.parse_sandbox_ref("url:h:1").id == "direct:h:1"
    bare = cua.parse_sandbox_ref("box")
    assert (bare.location, bare.id) == (None, "box")
    with pytest.raises(cua.CuaError.InvalidArgument):
        cua.parse_sandbox_ref("moon:x")
    # local= narrows a bare name.
    assert cua.qualify_sandbox_ref("box") == "box"
    assert cua.qualify_sandbox_ref("box", True) == "local:box"
    assert cua.qualify_sandbox_ref("box", False) == "cloud:box"
    with pytest.raises(cua.CuaError.InvalidArgument):
        cua.qualify_sandbox_ref("cloud:box", True)
    # The typed ambiguity error lists the qualified candidates.
    e = cua.CuaError.AmbiguousSandbox('"box" names 2 sandboxes; use one of: local:box, cloud:box')
    assert isinstance(e, cua.CuaError)
    assert e.candidates == ["local:box", "cloud:box"]


def test_listing_and_lookup_use_refs(tmp_path):
    c = cua.embedded(
        state_dir=str(tmp_path / "sandboxes"),
        spaces_home=str(tmp_path / "cua"),
        fleet_pool_home=str(tmp_path / "pools"),
        fleet_from_env=False,
        fleet_from_session=False,
    )
    import asyncio

    async def go():
        sbx = c.sandboxes()
        # A cloud ref needs Fleet; a bare name searches what is configured.
        with pytest.raises(cua.CuaError.ProviderNotConfigured):
            await sbx.get("cloud:nope")
        with pytest.raises(cua.CuaError.NotFound):
            await sbx.get("nope")
        with pytest.raises(cua.CuaError.InvalidArgument):
            await sbx.get("moon:x")

    asyncio.run(go())

"""Unified sandbox refs in cua-sandbox (hermetic): parsing through the
native core, narrowing with ``local=``, the typed ambiguity error, and
qualified ids on sandboxes and listings. Nothing here starts a sandbox or
reaches a cloud: the cloud lookup is patched."""

from __future__ import annotations

import importlib

import pytest
from cua_sandbox import InvalidArgument, Sandbox, Unsupported, _refs
from cua_sandbox._ended import CloudEnded
from cua_sandbox.transport.env import EnvTransport

sandbox_module = importlib.import_module("cua_sandbox.sandbox")


def test_refs_parse_with_the_legacy_spellings():
    for ref in ("local:box", "cloud:box", "direct:10.0.0.5:3211", "relay:0123abcd4567ef89"):
        assert _refs.parse(ref).id == ref
    assert _refs.parse("box") == _refs.Ref(None, "box")
    assert _refs.parse("url:h:1") == _refs.Ref("direct", "h:1")
    with pytest.raises(InvalidArgument):
        _refs.parse("moon:x")


async def test_resolve_qualified_narrowed_and_bare_names():
    is_local = {"box", "only-local"}.__contains__

    assert await _refs.resolve("local:box", None) == ("box", True, None)
    assert await _refs.resolve("direct:h:1", None) == ("h:1", None, "http://h:1")
    assert await _refs.resolve("only-local", None, is_local=is_local) == ("only-local", True, None)
    assert await _refs.resolve("elsewhere", None, is_local=is_local) == ("elsewhere", False, None)
    # local= narrows a bare name.
    assert await _refs.resolve("box", True, is_local=is_local) == ("box", True, None)
    # Cua Cloud has ended: a cloud ref, or a bare name narrowed to the cloud, raises.
    with pytest.raises(CloudEnded):
        await _refs.resolve("cloud:box", None)
    with pytest.raises(CloudEnded):
        await _refs.resolve("box", False, is_local=is_local)
    with pytest.raises(InvalidArgument):
        await _refs.resolve("local:box", False)
    with pytest.raises(Unsupported):
        await _refs.resolve("relay:0123abcd4567ef89", None)


async def test_lifecycle_calls_take_refs(monkeypatch):
    seen = []

    async def fake_delete_local(cls, name):
        seen.append(("local", name))

    monkeypatch.setattr(Sandbox, "_delete_local", classmethod(fake_delete_local))
    await Sandbox.delete("local:box")
    await Sandbox.delete("space://local/box")
    assert seen == [("local", "box"), ("local", "box")]
    with pytest.raises(Unsupported):
        await Sandbox.delete("direct:h:1")


def test_sandbox_ids_are_qualified():
    sb = Sandbox(EnvTransport(url="http://10.0.0.5:3211"), name="dev", _telemetry_enabled=False)
    assert (sb.id, sb.location) == ("local:dev", "local")
    sb._direct_url = "http://10.0.0.5:3211"
    assert (sb.id, sb.location) == ("direct:10.0.0.5:3211", "direct")

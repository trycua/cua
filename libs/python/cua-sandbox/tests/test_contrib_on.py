"""``on=`` (the CLI's ``--on``): local, cloud, or a contrib provider through
:class:`ContribRuntime`. Pure: nothing is created."""

from __future__ import annotations

import importlib

import pytest
from cua_sandbox import _placement
from cua_sandbox._sdk import InvalidArgument, InvalidPlacement
from cua_sandbox.image import Image
from cua_sandbox.runtime import contrib

# The module (the package exports a `sandbox` function of the same name).
sbmod = importlib.import_module("cua_sandbox.sandbox")


@pytest.fixture(autouse=True)
def _locations(monkeypatch):
    monkeypatch.setattr(contrib, "contrib_locations", lambda: ["e2b", "daytona", "modal"])


def apply(on, **kw):
    args = dict(
        local=None,
        runtime=None,
        cloud=None,
        pool=None,
        cpu=None,
        memory_mb=None,
        time_to_start=None,
    )
    args.update(kw)
    local, runtime = args.pop("local"), args.pop("runtime")
    return sbmod._apply_on(on, local, runtime, **args)


def test_location_words_pass_through_to_the_placement_model():
    # `_apply_on` only routes contrib providers; `local`, `cloud`, `fleet`,
    # `direct:` and `relay:` reach `_placement.resolve` unchanged.
    for on in (None, "local", "cloud", "fleet", "direct:10.0.0.5:3211", "relay:abc"):
        assert apply(on) == (on, None, None)
    assert apply("local", local=False) == ("local", False, None)
    # ...which is where contradictions are refused.
    with pytest.raises(InvalidArgument, match="contradict"):
        _placement.resolve(
            on="local",
            local=False,
            kind=None,
            runtime=None,
            cloud=None,
            cloud_only=[],
            image_kind=None,
        )


def test_a_contrib_word_runs_through_the_contrib_runtime():
    on, local, runtime = apply("E2B", cpu=4, memory_mb=8192, time_to_start=300)
    assert on is None
    assert local is True
    assert isinstance(runtime, contrib.ContribRuntime)
    assert (runtime.on, runtime.provider_kind, runtime.runtime_type) == ("e2b", "CONTRIB", "e2b")
    assert (runtime.cpus, runtime.memory_mb, runtime.ready_timeout) == (4, 8192, 300.0)


def test_contrib_refuses_conflicting_location_options():
    for kw in ({"local": True}, {"pool": "p"}, {"cloud": object()}):
        with pytest.raises(InvalidArgument, match="picks the provider"):
            apply("daytona", **kw)
    # An unknown word is not a contrib provider: `_apply_on` leaves it to
    # the placement model, which refuses it with the valid locations.
    assert apply("nosuchcloud") == ("nosuchcloud", None, None)
    with pytest.raises(InvalidPlacement, match="unknown location"):
        _placement.resolve(
            on="nosuchcloud",
            local=None,
            kind=None,
            runtime=None,
            cloud=None,
            cloud_only=[],
            image_kind=None,
        )


@pytest.mark.asyncio
async def test_contrib_image_refs_keep_the_variant():
    rt = contrib.ContribRuntime("e2b")
    assert await rt._image_ref(Image.from_registry("ghcr.io/trycua/linux:24.04"), "x") == (
        "ghcr.io/trycua/linux:24.04"
    )
    vm = Image.from_registry("ghcr.io/trycua/linux:24.04", kind="vm")
    assert await rt._image_ref(vm, "x") == "vm:ghcr.io/trycua/linux:24.04"

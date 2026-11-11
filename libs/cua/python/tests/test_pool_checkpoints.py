"""Checkpoints of Spaces on any of your machines, through the Python binding:
`Spaces.create` with image ``checkpoint:<name>`` (a fork, started where the
checkpoint is), `Sandboxes.checkpoint` / `checkpoints` / `delete_checkpoint`
across machines and `CheckpointSummary.host`.

Hermetic: an embedded runtime with a private home and no other machines, so
nothing starts; the refusals show the request reached the Spaces runtime as
a fork (not an image pull). Forks across machines run in the AWS pool e2e.
"""

from __future__ import annotations

import asyncio

import pytest

import cua


def run(coro):
    return asyncio.run(asyncio.wait_for(coro, timeout=60))


def test_checkpoint_summary_says_where_it_is():
    c = cua.CheckpointSummary(
        name="world",
        source="baker",
        runtime_type="qemu",
        created_at="2026-10-21T00:00:00Z",
        image="ghcr.io/trycua/linux:24.04",
        pinned=False,
        shared_memory=True,
        release_with_forks=False,
        forks=0,
        host="kvm-a",
    )
    assert c.host == "kvm-a"


def test_a_checkpoint_image_is_a_fork_not_an_image(tmp_path):
    c = cua.embedded(
        state_dir=str(tmp_path / "sbx"),
        spaces_home=str(tmp_path / "home"),
    )

    async def go():
        with pytest.raises(cua.CuaError.NotFound) as nowhere:
            await c.spaces().create(cua.SpaceCreateOptions(image="checkpoint:nowhere"))
        assert "no machine of yours holds it" in str(nowhere.value)
        with pytest.raises(cua.CuaError.InvalidArgument) as refused:
            await c.spaces().create(
                cua.SpaceCreateOptions(image="checkpoint:world", command=["true"])
            )
        assert "takes no command" in str(refused.value)
        with pytest.raises(cua.CuaError.InvalidArgument):
            await c.spaces().create(cua.SpaceCreateOptions(image="checkpoint:world", on="cloud"))
        assert await c.sandboxes().checkpoints() == []
        with pytest.raises(cua.CuaError.NotFound):
            await c.sandboxes().delete_checkpoint("nowhere")
        with pytest.raises(cua.CuaError.NotFound):
            await c.sandboxes().checkpoint("no-such-space", "cp")

    run(go())

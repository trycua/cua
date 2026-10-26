"""The checkpoint and fork API of the Python binding (hermetic: signatures
and records only, nothing is created), so it cannot drift from the Rust
SDK it is generated from."""

from __future__ import annotations

import inspect

import cua


def _params(fn) -> dict[str, inspect.Parameter]:
    return dict(inspect.signature(fn).parameters)


def test_sandboxes_checkpoint_takes_pin_defaulting_to_false():
    p = _params(cua.Sandboxes.checkpoint)
    assert list(p)[1:] == ["name", "checkpoint", "pin"]
    assert p["pin"].default is False
    assert inspect.iscoroutinefunction(cua.Sandboxes.checkpoint)


def test_sandboxes_pin_and_prune_checkpoints():
    assert list(_params(cua.Sandboxes.pin_checkpoint))[1:] == ["name", "pinned"]
    assert list(_params(cua.Sandboxes.prune_checkpoints))[1:] == []
    assert inspect.iscoroutinefunction(cua.Sandboxes.pin_checkpoint)
    assert inspect.iscoroutinefunction(cua.Sandboxes.prune_checkpoints)


def test_space_checkpoints_and_forks():
    p = _params(cua.Space.checkpoint)
    assert list(p)[1:] == ["checkpoint", "pin"]
    assert p["pin"].default is False
    assert list(_params(cua.Space.fork))[1:] == ["name"]
    assert list(_params(cua.Spaces.fork))[1:] == ["source", "name"]
    for fn in (cua.Space.checkpoint, cua.Space.fork, cua.Spaces.fork):
        assert inspect.iscoroutinefunction(fn), fn


def test_checkpoint_summary_says_pinned_and_shared_memory():
    c = cua.CheckpointSummary(
        name="world",
        source="bake",
        runtime_type="qemu",
        created_at="2026-10-19T00:00:00Z",
        image="ghcr.io/trycua/linux:24.04",
        pinned=True,
        shared_memory=True,
        release_with_forks=False,
        forks=0,
        host="",
    )
    assert (c.pinned, c.shared_memory) == (True, True)


def test_create_options_take_forkable():
    assert cua.SandboxCreateOptions(image="linux").forkable is None
    assert cua.SandboxCreateOptions(image="linux", forkable=True).forkable is True
    assert cua.SpaceCreateOptions().forkable is None
    assert cua.SpaceCreateOptions(forkable=True).forkable is True

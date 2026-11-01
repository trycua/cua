"""Space labels through the generated binding: the options and SpaceInfo
types, and the limits checked before anything is created. Creating a
labelled Space needs a VM; the core test `labels_are_kept_with_the_space_and_leave_with_it`
(crates/cua-spaces/tests/generic.rs) and the live runs cover that."""

from __future__ import annotations

import pytest
from test_smoke import run

import cua

TOO_MANY = {f"k{i}": "v" for i in range(65)}


async def exercise(c, fx):
    spaces = c.spaces()
    info = await spaces.add(fx["spaces_url"], fx["spaces_token"], "py-labels")
    assert info.labels == {}
    assert [s.labels for s in await spaces.list()] == [{}]

    opts = cua.SpaceCreateOptions(labels={"team": "research", "run": "7"})
    assert opts.labels == {"team": "research", "run": "7"}
    assert cua.SpaceCreateOptions().labels == {}

    bad = {
        "empty key": {"": "v"},
        "empty value": {"k": ""},
        "long key": {"k" * 64: "v"},
        "long value": {"k": "v" * 257},
        "too many": TOO_MANY,
    }
    for what, labels in bad.items():
        with pytest.raises(cua.CuaError.InvalidArgument):
            await spaces.create(cua.SpaceCreateOptions(on="local", labels=labels))
        assert what
    # A durable or reused Space cannot take labels.
    for extra in ({"durable": True}, {"reuse": True}):
        with pytest.raises(cua.CuaError.InvalidArgument, match="labels"):
            await spaces.create(
                cua.SpaceCreateOptions(on="local", labels={"a": "b"}, **extra)
            )
    await spaces.delete(info.id)


def test_labels_types_and_limits(fixtures, tmp_path):
    c = cua.embedded(
        state_dir=str(tmp_path / "sbx"),
        spaces_home=str(tmp_path / "cua"),
    )
    run(exercise(c, fixtures))

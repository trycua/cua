"""When a Space was created and when it ends, read back through the
generated binding (SpaceInfo.created_at_unix / expires_at_unix / on_expire),
against the fixture's in-process cua-spacesd core. No VMs."""

from __future__ import annotations

import time

import pytest
from test_smoke import run

import cua


async def exercise(c, fx):
    spaces = c.spaces()
    t0 = int(time.time())
    info = await spaces.add(fx["spaces_url"], fx["spaces_token"], "py-clock")
    # A Space added by address was "created" when it was added.
    assert t0 - 2 <= info.created_at_unix <= int(time.time()) + 2
    assert info.expires_at_unix == 0 and info.on_expire == ""

    at = await spaces.set_lifetime(info.id, 60, None)
    row = (await spaces.list())[0]
    assert (row.expires_at_unix, row.on_expire) == (at, "delete")
    assert row.created_at_unix == info.created_at_unix
    assert t0 + 55 <= row.expires_at_unix <= t0 + 65
    assert (await spaces.resolve(info.id)).expires_at_unix == at

    # A later call replaces both the time and the action.
    at = await spaces.set_lifetime(info.id, 600, "suspend")
    row = (await spaces.list())[0]
    assert (row.expires_at_unix, row.on_expire) == (at, "suspend")
    # Keeping the action: an empty on_expire keeps "suspend".
    await spaces.set_lifetime(info.id, 300, None)
    assert (await spaces.list())[0].on_expire == "suspend"

    # The types: SpaceInfo carries three integers and a string.
    assert isinstance(row.created_at_unix, int) and isinstance(row.on_expire, str)
    with pytest.raises(cua.CuaError.InvalidArgument):
        await spaces.set_lifetime(info.id, 1, "explode")
    await spaces.delete(info.id)


def test_created_and_expires_are_listed(fixtures, tmp_path):
    c = cua.embedded(
        state_dir=str(tmp_path / "sbx"),
        spaces_home=str(tmp_path / "cua"),
    )
    run(exercise(c, fixtures))

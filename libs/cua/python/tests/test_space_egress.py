"""Egress policies through the generated binding: the types (the policy and
where it is enforced), the options, the input validation and the
refusals, all decided before anything runs. Enforcement itself is the
host's (nothing here touches a firewall); the rules are covered by the unit
tests in crates/cua-vmm/src/egress.rs and the opt-in runs in
crates/cua-daemon/tests/space_egress.rs prove them against a guest root."""

from __future__ import annotations

import pytest
from test_smoke import run

import cua


async def exercise(c, fx):
    spaces = c.spaces()
    info = await spaces.add(fx["spaces_url"], fx["spaces_token"], "py-egress")

    # The types.
    assert info.egress == cua.EgressPolicy(allow=[], deny=[])
    assert info.egress_enforced is None
    assert cua.EgressEnforcement.HOST != None  # noqa: E711 (the enum exists)
    assert cua.EgressPolicy().allow == [] and cua.EgressPolicy().deny == []
    policy = cua.EgressPolicy(allow=["1.1.1.1", "2606:4700::1111"], deny=["all"])
    assert (policy.allow, policy.deny) == (["1.1.1.1", "2606:4700::1111"], ["all"])
    assert cua.SpaceCreateOptions().egress is None
    assert cua.SpaceCreateOptions(egress=policy).egress == policy

    # Entries are IP addresses and CIDRs: refused before a create or a call.
    for bad in (["example.com"], ["*.example.com"], ["10.0.0.0/33"], ["1.2.3"], [""]):
        with pytest.raises(cua.CuaError.InvalidArgument):
            await spaces.set_egress(info.id, cua.EgressPolicy(deny=bad))
        with pytest.raises(cua.CuaError.InvalidArgument):
            await spaces.set_egress(info.id, cua.EgressPolicy(allow=bad))
        with pytest.raises(cua.CuaError.InvalidArgument):
            await spaces.create(
                cua.SpaceCreateOptions(on="local", egress=cua.EgressPolicy(deny=bad))
            )
    with pytest.raises(cua.CuaError.InvalidArgument, match="by IP/CIDR"):
        await spaces.set_egress(info.id, cua.EgressPolicy(deny=["example.com"]))
    # Not with durable or reuse.
    for extra in ({"durable": True}, {"reuse": True}):
        with pytest.raises(cua.CuaError.InvalidArgument, match="egress"):
            await spaces.create(
                cua.SpaceCreateOptions(
                    on="local", egress=cua.EgressPolicy(deny=["all"]), **extra
                )
            )
    # Cua Cloud has ended: `on="cloud"` is refused before anything is made.
    with pytest.raises(cua.CuaError.Runtime, match="Cua Cloud has ended"):
        await spaces.create(
            cua.SpaceCreateOptions(on="cloud", egress=cua.EgressPolicy(deny=["all"]))
        )
    # A Space added by address is not run by cua: nothing to enforce it.
    with pytest.raises(cua.CuaError.Unsupported, match="not run by cua"):
        await spaces.set_egress(info.id, cua.EgressPolicy(deny=["all"]))
    listed = [s for s in await spaces.list() if s.id == info.id]
    assert listed and listed[0].egress_enforced is None
    with pytest.raises(cua.CuaError.NotFound):
        await spaces.set_egress("direct:127.0.0.1:9", cua.EgressPolicy(deny=["all"]))
    await spaces.delete(info.id)


def test_egress_types_and_validation(fixtures, tmp_path):
    c = cua.embedded(
        state_dir=str(tmp_path / "sbx"),
        spaces_home=str(tmp_path / "cua"),
    )
    run(exercise(c, fixtures))

"""`Sandboxes.bake`: the options record and the checks that run before any
VM starts (the bake itself is the docker tier's integration test,
libs/images/linux/tests/bake-world.sh)."""

from __future__ import annotations

import asyncio

import pytest

import cua


def test_bake_options_defaults() -> None:
    o = cua.BakeOptions(project_dir="/worlds/w", checkpoint="w1")
    assert o.image is None and o.name is None and o.dest is None
    assert o.compose_files == [] and o.image_tarballs == []
    assert o.health == [] and o.run == []
    assert o.health_timeout_s == 600
    assert not o.verify_fork and not o.keep


def test_bake_checks_before_starting_a_vm(tmp_path) -> None:
    c = cua.embedded(
        state_dir=str(tmp_path / "sandboxes"),
        spaces_home=str(tmp_path / "cua"),
    )

    async def go() -> None:
        sbx = c.sandboxes()
        with pytest.raises(cua.CuaError.NotFound):
            await sbx.bake(cua.BakeOptions(project_dir=str(tmp_path / "missing"), checkpoint="w1"))
        with pytest.raises(cua.CuaError.NotFound):
            await sbx.bake(
                cua.BakeOptions(
                    project_dir=str(tmp_path),
                    checkpoint="w1",
                    image_tarballs=[str(tmp_path / "missing.tar")],
                )
            )
        with pytest.raises(cua.CuaError.InvalidArgument):
            await sbx.bake(cua.BakeOptions(project_dir=str(tmp_path), checkpoint=" "))
        assert await sbx.list("local") == []

    asyncio.run(go())

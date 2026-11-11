"""`Spaces.build`: the options and result records, and the checks that run
before any Space starts (the build itself is cua-spaces' QEMU test and the
Mac run in the PR)."""

from __future__ import annotations

import asyncio

import pytest

import cua


def _embedded(tmp_path):
    return cua.embedded(
        state_dir=str(tmp_path / "sandboxes"),
        spaces_home=str(tmp_path / "cua"),
    )


def test_build_options_defaults_and_steps() -> None:
    o = cua.BuildOptions(base="ubuntu:24.04")
    assert o.dockerfile is None and o.tag is None and o.context_dir is None
    assert o.steps == [] and o.build_args == {}
    assert not o.no_cache
    run = cua.BuildStep.RUN(command="make")
    assert run.command == "make"
    copy = cua.BuildStep.COPY(
        sources=["a.txt"], dest="/srv/", owner=None, mode="640", unpack=False
    )
    assert copy.mode == "640"
    assert cua.SpaceCreateOptions(build=o).build.base == "ubuntu:24.04"
    assert cua.SpaceCreateOptions().build is None


def test_build_is_refused_before_anything_runs(tmp_path) -> None:
    spaces = _embedded(tmp_path).spaces()

    async def go() -> None:
        with pytest.raises(cua.CuaError.InvalidArgument):
            await spaces.build(cua.BuildOptions())
        with pytest.raises(cua.CuaError.InvalidArgument, match="line 3"):
            await spaces.build(cua.BuildOptions(dockerfile="FROM linux\nRUN true\nFROM linux\n"))
        with pytest.raises(cua.CuaError.InvalidArgument, match="missing.txt"):
            await spaces.build(
                cua.BuildOptions(
                    dockerfile="FROM linux\nCOPY missing.txt /x\n", context_dir=str(tmp_path)
                )
            )
        with pytest.raises(cua.CuaError.InvalidArgument, match="ubuntu"):
            await spaces.build(cua.BuildOptions(base="alpine:3"))
        assert await spaces.builds() == []
        with pytest.raises(cua.CuaError.NotFound):
            await spaces.delete_build("nope")

    asyncio.run(go())

"""Batched forks through the Python binding (`Sandboxes.fork_many`,
`fork_groups`, `publish_checkpoint`).

Hermetic: an embedded runtime with a private home, checkpoints that do not
exist, so every fork fails on its own and no VM starts. That exercises the
binding's records, the event stream and the summary; real forks across
machines are covered by the daemon's live suite.
"""

from __future__ import annotations

import asyncio
import inspect

import pytest

import cua


def run(coro):
    return asyncio.run(asyncio.wait_for(coro, timeout=60))


def test_records_have_their_defaults():
    o = cua.ForkManyOptions()
    assert o.on is None
    assert (o.group_size, o.max_parallel_per_host, o.cpus, o.memory_mb) == (0, 0, 0, 0)
    assert (o.retries, o.no_retry, o.name_prefix, o.fallback) == (0, False, "", None)
    g = cua.ForkGroup(source="world", count=8)
    assert g.name_prefix == ""
    f = cua.SnapshotFallbackOptions(dir="~/Cua Volume/snapshots")
    assert (f.s3_bucket, f.s3_prefix, f.s3_region, f.s3_endpoint) == ("", "", "", "")


def test_methods_and_signatures():
    sig = inspect.signature(cua.Sandboxes.fork_many)
    assert list(sig.parameters)[1:] == ["source", "count", "options"]
    assert list(inspect.signature(cua.Sandboxes.fork_groups).parameters)[1:] == [
        "groups",
        "options",
    ]
    assert list(inspect.signature(cua.Sandboxes.publish_checkpoint).parameters)[1:] == [
        "name",
        "fallback",
    ]
    for name in ("next", "collect"):
        assert inspect.iscoroutinefunction(getattr(cua.ForkBatch, name)), name


def test_fork_many_streams_each_fork_then_the_summary(tmp_path):
    c = cua.embedded(state_dir=str(tmp_path / "sbx"))

    async def go():
        batch = await c.sandboxes().fork_many(
            "no-such-checkpoint",
            3,
            # Counted small, so this machine has room for all three whatever
            # its memory (a CI runner's, or Windows' unmeasured default of
            # 4 GiB); the test is about the events, not about capacity.
            cua.ForkManyOptions(on="local", group_size=2, name_prefix="py", memory_mb=64),
        )
        events = []
        while (e := await batch.next()) is not None:
            events.append(e)
        return events

    events = run(go())
    placements = [e.placement for e in events if e.is_placement()]
    items = sorted((e.item for e in events if e.is_item()), key=lambda i: i.index)
    assert [p.host for p in placements] == ["local", "local"]
    assert [i.name for i in items] == ["py-g0-0", "py-g0-1", "py-g1-0"]
    assert all(i.error_code == "sync_failed" and i.space == "" for i in items)
    assert events[-1].is_done()
    summary = events[-1].summary
    assert (summary.ok, summary.failed) == (0, 3)


def test_collect_and_refusals(tmp_path):
    c = cua.embedded(state_dir=str(tmp_path / "sbx"))

    async def go():
        result = await (
            await c.sandboxes().fork_groups(
                [cua.ForkGroup(source="w-a", count=2, name_prefix="a")], None
            )
        ).collect()
        assert [i.name for i in result.items] == ["a-0", "a-1"]
        assert result.summary.failed == 2
        with pytest.raises(cua.CuaError.InvalidArgument):
            b = await c.sandboxes().fork_many("w", 0, None)
            await b.next()
        with pytest.raises(cua.CuaError.NotFound):
            await c.sandboxes().publish_checkpoint(
                "no-such-checkpoint", cua.SnapshotFallbackOptions(dir=str(tmp_path / "vol"))
            )

    run(go())

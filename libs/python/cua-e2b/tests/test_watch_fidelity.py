"""Directory watch behaviours pinned against e2b 2.58 (in-memory backend):
event shapes cua-spacesd reports differently, open/stop errors, the
timeout's effect on the task that opened the watch."""

from __future__ import annotations

import asyncio

import pytest
from e2b import (
    AsyncSandbox,
    FileNotFoundException,
    InvalidArgumentException,
    NotFoundException,
    Sandbox,
    TimeoutException,
)
from e2b._backend import FsEvent, set_backend
from fake_backend import FakeBackend, NotFound


@pytest.fixture
def backend():
    b = FakeBackend()
    set_backend(b)
    yield b
    set_backend(None)


async def _sb():
    return await AsyncSandbox.create(template="w")


async def _feed(backend, sb, events):
    m = backend.machines[sb.sandbox_id]
    m.watchers["/w"].extend(events)


async def test_a_move_inside_the_tree_is_rename_of_the_old_name_then_create(backend):
    sb = await _sb()
    seen = []
    h = await sb.files.watch_dir("/w", seen.append, timeout=0)
    # what cua-spacesd reports for `mv f.txt g.txt`
    await _feed(
        backend,
        sb,
        [
            FsEvent(path="/w/f.txt", type="remove"),
            FsEvent(path="/w/g.txt", type="create"),
            FsEvent(path="/w/g.txt", type="rename", old_path="/w/f.txt"),
        ],
    )
    await asyncio.sleep(0.6)
    await h.stop()
    assert [(e.type.value, e.name) for e in seen] == [("rename", "f.txt"), ("create", "g.txt")]


async def test_a_real_remove_and_a_rename_without_origin_stay_as_they_are(backend):
    sb = await _sb()
    seen = []
    h = await sb.files.watch_dir("/w", seen.append, timeout=0)
    await _feed(
        backend,
        sb,
        [FsEvent(path="/w/a", type="remove"), FsEvent(path="/w/b", type="rename")],
    )
    await asyncio.sleep(0.6)
    await h.stop()
    assert [(e.type.value, e.name) for e in seen] == [("remove", "a"), ("rename", "b")]


async def test_a_rename_whose_remove_came_earlier_still_names_the_old_entry(backend):
    sb = await _sb()
    seen = []
    h = await sb.files.watch_dir("/w", seen.append, timeout=0)
    await _feed(backend, sb, [FsEvent(path="/w/g", type="rename", old_path="/w/f")])
    await asyncio.sleep(0.6)
    await h.stop()
    assert [(e.type.value, e.name) for e in seen] == [("rename", "f")]


def test_sync_events_fold_renames_too(backend):
    sb = Sandbox.create(template="w")
    w = sb.files.watch_dir("/w")
    backend.machines[sb.sandbox_id].watchers["/w"].extend(
        [
            FsEvent(path="/w/f", type="remove"),
            FsEvent(path="/w/g", type="create"),
            FsEvent(path="/w/g", type="rename", old_path="/w/f"),
        ]
    )
    assert [(e.type.value, e.name) for e in w.get_new_events()] == [
        ("rename", "f"),
        ("create", "g"),
    ]


async def test_watch_open_errors_are_envds(backend):
    sb = await _sb()
    m = backend.machines[sb.sandbox_id]

    async def missing(path, recursive, user=None):
        raise NotFound(f"path not found: {path}: No such file or directory (os error 2)")

    m.watch = missing
    with pytest.raises(FileNotFoundException) as ei:
        await sb.files.watch_dir("/w/nope", lambda e: None)
    assert str(ei.value) == "path /w/nope not found: stat /w/nope: no such file or directory"

    from fake_backend import CuaError

    class Env(CuaError):
        pass

    async def notdir2(path, recursive, user=None):
        raise Env(f"env: not a directory: {path}: not a directory (FailedPrecondition)")

    m.watch = notdir2
    with pytest.raises(InvalidArgumentException) as ei:
        await sb.files.watch_dir("/etc/hostname", lambda e: None)
    assert str(ei.value) == "path /etc/hostname not a directory: %!w(<nil>)"
    with pytest.raises(InvalidArgumentException):
        Sandbox.connect(sb.sandbox_id).files.watch_dir("/etc/hostname")


def test_sync_stop_twice_is_not_found(backend):
    sb = Sandbox.create(template="w")
    w = sb.files.watch_dir("/w")
    m = backend.machines[sb.sandbox_id]

    async def gone(watcher_id):
        raise NotFound(f'session not found: unknown or expired watcher "{watcher_id}"')

    m.unwatch = gone
    with pytest.raises(NotFoundException) as ei:
        w.stop()
    assert str(ei.value) == "watcher with id /w not found"


async def test_watch_timeout_cancels_the_opening_task_and_reports_on_exit(backend):
    sb = await _sb()
    exits = []

    async def opener():
        await sb.files.watch_dir("/w", lambda e: None, on_exit=exits.append, timeout=0.3)
        await asyncio.sleep(5)

    t = asyncio.ensure_future(opener())
    await asyncio.wait([t], timeout=3)
    assert t.cancelled()
    await asyncio.sleep(0.2)
    assert len(exits) == 1 and isinstance(exits[0], TimeoutException)
    assert str(exits[0]).startswith("context deadline exceeded: This error is likely due to")


async def test_a_finished_opener_is_left_alone_when_the_watch_times_out(backend):
    sb = await _sb()
    exits = []

    async def opener():
        await sb.files.watch_dir("/w", lambda e: None, on_exit=exits.append, timeout=0.3)

    t = asyncio.ensure_future(opener())
    await t
    await asyncio.sleep(0.8)
    assert not t.cancelled() and len(exits) == 1

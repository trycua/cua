"""AsyncWatchHandle: a cua-spacesd directory watcher, polled."""

from __future__ import annotations

import asyncio
import inspect
import posixpath
from typing import Awaitable, Callable, List, Optional

from e2b._errors import map_error
from e2b.exceptions import (
    FileNotFoundException,
    InvalidArgumentException,
    NotFoundException,
    TimeoutException,
)
from e2b.sandbox.filesystem.filesystem import EntryInfo
from e2b.sandbox.filesystem.watch_handle import FilesystemEvent, FilesystemEventType

POLL_SECONDS = 0.2

WATCH_DEADLINE = (
    "context deadline exceeded: This error is likely due to exceeding 'timeout' — the total time a "
    "long running request (like process or directory watch) can be active — or "
    "'request_timeout'. You can modify these by passing 'timeout' or 'request_timeout' when "
    "making the request. Use '0' to disable the timeout."
)


def fold_renames(raw) -> list:
    """cua-spacesd reports a move inside the watched tree as three events
    (REMOVE old, CREATE new, RENAME new with `old_path`); E2B reports two:
    RENAME with the old name, then CREATE of the new one. Folds the triple
    (the REMOVE of the old path turns into the RENAME, the RENAME of the new
    path is dropped). A rename without `old_path` (a kernel that does not
    say where it came from) is kept as it is. A move out of the tree is a
    plain REMOVE here, as is `rm`: cua-spacesd reports no difference."""
    out: list = []
    for e in raw:
        if e.type == "rename" and getattr(e, "old_path", ""):
            renamed = type(e)(path=e.old_path, type="rename")
            i = next(
                (
                    i
                    for i in range(len(out) - 1, -1, -1)
                    if out[i].type == "remove" and out[i].path == e.old_path
                ),
                None,
            )
            if i is not None:
                out[i] = renamed
            else:
                # the REMOVE went out with an earlier poll: the rename still
                # names the old entry, as E2B does
                out.append(renamed)
            continue
        out.append(e)
    return out


async def to_events(
    machine, raw, root: str, entry_of: Optional[Callable[[str], Awaitable[Optional[EntryInfo]]]]
) -> List[FilesystemEvent]:
    """Watcher events (paths from the transport) as e2b `FilesystemEvent`s:
    `name` relative to the watched directory; with `entry_of`, the affected
    entry as it is now (`None` where it no longer exists)."""
    out: List[FilesystemEvent] = []
    for e in fold_renames(raw):
        name = posixpath.relpath(e.path, root) if e.path.startswith("/") else e.path
        entry = None
        if entry_of is not None and e.type not in ("remove", "rename"):
            entry = await entry_of(e.path)
        out.append(FilesystemEvent(name=name, type=FilesystemEventType(e.type), entry=entry))
    return out


def watch_open_error(e: BaseException, path: str) -> BaseException:
    """The error E2B raises when a directory cannot be watched: a missing
    path is a `FileNotFoundException`, a path that is no directory an
    `InvalidArgumentException` (envd's texts, including its unformatted
    `%!w(<nil>)`)."""
    mapped = map_error(e, file=True)
    if isinstance(mapped, FileNotFoundException):
        return FileNotFoundException(
            f"path {path} not found: stat {path}: no such file or directory"
        )
    if "not a directory" in str(mapped).lower():
        return InvalidArgumentException(f"path {path} not a directory: %!w(<nil>)")
    return mapped


def watcher_gone_error(e: BaseException, watcher_id: str) -> BaseException:
    """E2B's error for a watcher that does not exist (any more)."""
    if type(e).__name__ == "NotFound" and "watcher" in str(e).lower():
        return NotFoundException(f"watcher with id {watcher_id} not found")
    return map_error(e)


class AsyncWatchHandle:
    """Watches a directory; `stop()` ends it. `on_exit` fires exactly once
    when the watch ends: with the error that ended it (a `TimeoutException`
    when `timeout` ran out), or `None` when `stop()` ended it."""

    def __init__(
        self,
        machine,
        watcher_id: str,
        root: str,
        on_event,
        on_exit=None,
        timeout=None,
        entry_of=None,
        opener=None,
    ):
        self._m = machine
        self._id = watcher_id
        self._root = root
        self._on_event = on_event
        self._on_exit = on_exit
        self._timeout = timeout
        self._entry_of = entry_of
        self._opener = opener
        self._task = asyncio.create_task(self._run())

    async def _call(self, cb, value):
        if cb is None:
            return
        r = cb(value)
        if inspect.isawaitable(r):
            await r

    async def _call_on_exit(self, error: Optional[BaseException]) -> None:
        # The terminal callback: an error it raises has nowhere to go.
        try:
            await self._call(self._on_exit, error)
        except Exception:  # noqa: BLE001
            pass

    async def _run(self):
        error: Optional[BaseException] = None
        loop = asyncio.get_running_loop()
        deadline = loop.time() + self._timeout if self._timeout else None
        try:
            while True:
                if deadline is not None and loop.time() >= deadline:
                    error = TimeoutException(WATCH_DEADLINE)
                    # E2B's SDK runs the stream's timer in the task that
                    # opened it, so a watch that times out cancels that task
                    # (recorded as `opener_task: cancelled`)
                    if self._opener is not None and not self._opener.done():
                        self._opener.cancel()
                    break
                raw = await self._m.watch_events(self._id)
                for ev in await to_events(self._m, raw, self._root, self._entry_of):
                    await self._call(self._on_event, ev)
                await asyncio.sleep(POLL_SECONDS)
        except asyncio.CancelledError:
            # `stop()` ends the watch: a clean end, `on_exit(None)`, and the
            # cancellation still finishes the task.
            await self._unwatch()
            await self._call_on_exit(None)
            raise
        except Exception as e:  # noqa: BLE001
            error = map_error(e)
        await self._unwatch()
        await self._call_on_exit(error)

    async def _unwatch(self) -> None:
        try:
            await asyncio.shield(self._m.unwatch(self._id))
        except Exception:  # noqa: BLE001
            pass

    async def stop(self):
        """Stops watching."""
        await asyncio.sleep(0)  # a watch stopped at once still starts, so it can end
        self._task.cancel()
        await asyncio.wait([self._task])

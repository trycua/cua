"""WatchHandle: the synchronous directory watch, polled with `get_new_events`."""

from __future__ import annotations

from typing import List, Optional

from e2b.exceptions import SandboxException
from e2b.sandbox.filesystem.watch_handle import FilesystemEvent
from e2b.sandbox_async.filesystem.watch_handle import to_events, watcher_gone_error
from e2b.sandbox_sync._loop import Loop


class WatchHandle:
    """Handle for watching filesystem events: `get_new_events()` returns what
    happened since the last call; `stop()` ends the watch."""

    def __init__(self, machine, watcher_id: str, root: str, entry_of=None):
        self._m = machine
        self._id = watcher_id
        self._root = root
        self._entry_of = entry_of
        self._closed = False

    def stop(self, request_timeout: Optional[float] = None):
        """Stops watching; no more events can be fetched."""
        try:
            Loop.run(self._m.unwatch(self._id))
        except Exception as e:  # noqa: BLE001
            raise watcher_gone_error(e, self._id)
        self._closed = True

    def get_new_events(self, request_timeout: Optional[float] = None) -> List[FilesystemEvent]:
        """The events since the last call (or since the watch began)."""
        if self._closed:
            raise SandboxException("The watcher is already stopped")

        async def fetch():
            raw = await self._m.watch_events(self._id)
            return await to_events(self._m, raw, self._root, self._entry_of)

        try:
            return Loop.run(fetch())
        except Exception as e:  # noqa: BLE001
            raise watcher_gone_error(e, self._id)

"""Filesystem: the synchronous `sandbox.files`."""

from __future__ import annotations

from typing import IO, Dict, List, Literal, Optional, Union, overload

from e2b._errors import map_error
from e2b.sandbox.filesystem.filesystem import (
    EntryInfo,
    FileStreamReader,
    WriteEntry,
    WriteInfo,
)
from e2b.sandbox_async.commands.command import _bounded, guest_user
from e2b.sandbox_async.filesystem.filesystem import Filesystem as AsyncFilesystem
from e2b.sandbox_async.filesystem.watch_handle import watch_open_error
from e2b.sandbox_sync._loop import Loop, proxy
from e2b.sandbox_sync.filesystem.watch_handle import WatchHandle

Username = str


class Filesystem:
    """Reads and writes files in the sandbox."""

    def __init__(self, inner: AsyncFilesystem):
        self._f = inner

    write = proxy("_f", AsyncFilesystem.write)
    write_files = proxy("_f", AsyncFilesystem.write_files)
    list = proxy("_f", AsyncFilesystem.list)
    exists = proxy("_f", AsyncFilesystem.exists)
    get_info = proxy("_f", AsyncFilesystem.get_info)
    remove = proxy("_f", AsyncFilesystem.remove)
    rename = proxy("_f", AsyncFilesystem.rename)
    make_dir = proxy("_f", AsyncFilesystem.make_dir)

    @overload
    def read(
        self,
        path: str,
        format: Literal["text"] = "text",
        user: Optional[Username] = None,
        request_timeout: Optional[float] = None,
        gzip: bool = False,
    ) -> str: ...

    @overload
    def read(
        self,
        path: str,
        format: Literal["bytes"],
        user: Optional[Username] = None,
        request_timeout: Optional[float] = None,
        gzip: bool = False,
    ) -> bytearray: ...

    @overload
    def read(
        self,
        path: str,
        format: Literal["stream"],
        user: Optional[Username] = None,
        request_timeout: Optional[float] = None,
        gzip: bool = False,
        stream_idle_timeout: Optional[float] = None,
    ) -> FileStreamReader: ...

    def read(
        self,
        path: str,
        format: Literal["text", "bytes", "stream"] = "text",
        user: Optional[Username] = None,
        request_timeout: Optional[float] = None,
        gzip: bool = False,
        stream_idle_timeout: Optional[float] = None,
    ):
        """Reads a file as `str`, `bytearray`, or a `FileStreamReader` (an
        iterator of chunks; use it as a context manager or consume it)."""
        if format != "stream":
            return Loop.run(self._f.read(path, format, user, request_timeout, gzip))
        reader = Loop.run(
            self._f.read(path, "stream", user, request_timeout, gzip, stream_idle_timeout)
        )

        def chunks():
            while True:
                try:
                    yield Loop.run(reader.__anext__())
                except StopAsyncIteration:
                    return

        return FileStreamReader(chunks(), close=lambda: Loop.run(reader.aclose()))

    def watch_dir(
        self,
        path: str,
        user: Optional[Username] = None,
        request_timeout: Optional[float] = None,
        recursive: bool = False,
        include_entry: bool = False,
        allow_network_mounts: bool = False,
    ) -> WatchHandle:
        """Starts watching `path`; poll the returned handle with
        `get_new_events()`."""
        m = self._f._m

        async def start():
            p = await self._f._abs(path, user)
            watcher = await _bounded(m.watch(p, recursive, guest_user(user)), request_timeout)

            async def entry_of(x: str):
                try:
                    from e2b.sandbox_async.filesystem.filesystem import _entry

                    return _entry(await m.stat(x, guest_user(user)))
                except Exception:  # noqa: BLE001
                    return None

            return p, watcher, entry_of if include_entry else None

        try:
            root, watcher, entry_of = Loop.run(start())
        except Exception as e:  # noqa: BLE001
            raise watch_open_error(e, path)
        return WatchHandle(m, watcher, root, entry_of)


__all__ = ["Filesystem"]

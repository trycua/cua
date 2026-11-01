"""Filesystem types and pure helpers, as in e2b.sandbox.filesystem.filesystem."""

from __future__ import annotations

import asyncio
import re
from dataclasses import dataclass, field
from datetime import datetime
from enum import Enum
from typing import IO, AsyncIterator, Dict, Iterator, Optional, TypedDict, Union

from e2b.exceptions import InvalidArgumentException


class FileType(Enum):
    """Type of a filesystem object."""

    FILE = "file"
    DIR = "dir"
    SYMLINK = "symlink"


def map_file_type_str(value: Optional[str]) -> Optional[FileType]:
    """A file type word (`"file"`, `"dir"`) as `FileType`; `None` when unknown."""
    if value == FileType.FILE.value:
        return FileType.FILE
    elif value == FileType.DIR.value:
        return FileType.DIR
    return None


def map_metadata(metadata) -> Optional[Dict[str, str]]:
    """A metadata map as a plain dict; `None` when empty."""
    if not metadata:
        return None
    return dict(metadata)


@dataclass
class WriteInfo:
    """Sandbox filesystem object information."""

    name: str
    type: Optional[FileType]
    path: str
    metadata: Optional[Dict[str, str]] = field(default=None, kw_only=True)
    """User-defined metadata kept on the file as `user.e2b.*` extended
    attributes. On writes it is what was supplied; on reads (`get_info`,
    `list`, `rename`) any `user.e2b.*` attribute on the file. `None` when none
    is set."""

    @classmethod
    def from_dict(cls, payload: Dict) -> "WriteInfo":
        """A `WriteInfo` from an upload response entry."""
        return cls(
            name=payload["name"],
            type=map_file_type_str(payload.get("type")),
            path=payload["path"],
            metadata=map_metadata(payload.get("metadata")),
        )


@dataclass
class EntryInfo(WriteInfo):
    """Extended sandbox filesystem object information."""

    size: int
    mode: int
    permissions: str
    owner: str
    group: str
    modified_time: datetime
    symlink_target: Optional[str] = None


class WriteEntry(TypedDict):
    """The path and data of one file for `files.write_files`."""

    path: str
    data: Union[str, bytes, IO]


METADATA_HEADER_PREFIX = "X-Metadata-"

# Metadata keys travel as `X-Metadata-<key>` header names, so they must be
# valid HTTP tokens; values are printable US-ASCII.
_METADATA_KEY_REGEX = re.compile(r"\A[A-Za-z0-9!#$%&'*+\-.^_`|~]+\Z")
_METADATA_VALUE_REGEX = re.compile(r"\A[\x20-\x7e]*\Z")


def validate_metadata(metadata: Optional[Dict[str, str]]) -> None:
    """Validates file metadata keys and values (the texts are e2b's)."""
    if not metadata:
        return
    for key, value in metadata.items():
        if not _METADATA_KEY_REGEX.match(key):
            raise InvalidArgumentException(
                f"Invalid metadata key {key!r}: keys must be non-empty and use only "
                "HTTP token characters (letters, digits and !#$%&'*+-.^_`|~)."
            )
        if not _METADATA_VALUE_REGEX.match(value):
            raise InvalidArgumentException(
                f"Invalid metadata value for key {key!r}: values must be printable US-ASCII."
            )


def metadata_to_headers(metadata: Optional[Dict[str, str]]) -> Dict[str, str]:
    """File metadata as the `X-Metadata-*` header map e2b's upload sends."""
    if not metadata:
        return {}
    return {f"{METADATA_HEADER_PREFIX}{key}": value for key, value in metadata.items()}


def _read_timeout(message: str) -> BaseException:
    """e2b raises `httpx.ReadTimeout` for a stalled stream; use it when httpx
    is installed (code written for e2b catches it), else the builtin
    `TimeoutError`."""
    try:
        import httpx

        return httpx.ReadTimeout(message)
    except ImportError:
        return TimeoutError(message)


class AsyncFileStreamReader(AsyncIterator[bytes]):
    """Async iterator over a file's chunks, as returned by
    `files.read(format="stream")`.

    One streamed read from cua-spacesd (it resumes where a dropped stream
    stopped and checks each segment's SHA-256), buffered a few chunks ahead:
    memory stays bounded whatever the file size. Consume it fully, use it as
    an async context manager, or call `aclose()`. A stream that delivers
    nothing for `idle_timeout` seconds raises `httpx.ReadTimeout` (the
    builtin `TimeoutError` without httpx), as e2b's does.
    """

    def __init__(self, download, idle_timeout: Optional[float] = None):
        self._d = download
        self._idle = idle_timeout
        self._closed = False

    def __aiter__(self) -> AsyncIterator[bytes]:
        return self

    async def __anext__(self) -> bytes:
        try:
            nxt = self._d.next_chunk()
            chunk = await (asyncio.wait_for(nxt, self._idle) if self._idle else nxt)
        except asyncio.TimeoutError as e:
            await self.aclose()
            raise _read_timeout(f"no data for {self._idle} s") from e
        except BaseException:
            await self.aclose()
            raise
        if chunk is None:
            await self.aclose()
            raise StopAsyncIteration
        return chunk

    async def aclose(self) -> None:
        """Stops the transfer. Safe to call more than once."""
        if self._closed:
            return
        self._closed = True
        self._d.cancel()

    async def __aenter__(self) -> "AsyncFileStreamReader":
        return self

    async def __aexit__(self, *exc_info) -> None:
        await self.aclose()


class FileStreamReader(Iterator[bytes]):
    """Iterator over a file's chunks, as returned by the synchronous
    `files.read(format="stream")`. Consume it fully, use it as a context
    manager, or call `close()`."""

    def __init__(self, chunks: Iterator[bytes], close=None):
        self._it = chunks
        self._close = close
        self._closed = False

    def __iter__(self) -> Iterator[bytes]:
        return self

    def __next__(self) -> bytes:
        try:
            return next(self._it)
        except BaseException:
            self.close()
            raise

    def close(self) -> None:
        """Stops the transfer. Safe to call more than once."""
        if self._closed:
            return
        self._closed = True
        if self._close is not None:
            self._close()

    def __enter__(self) -> "FileStreamReader":
        return self

    def __exit__(self, *exc_info) -> None:
        self.close()

"""Filesystem watch types, as in e2b.sandbox.filesystem.watch_handle."""

from dataclasses import dataclass
from enum import Enum
from typing import Optional

from e2b.sandbox.filesystem.filesystem import EntryInfo


class FilesystemEventType(Enum):
    """A filesystem event's kind."""

    CHMOD = "chmod"
    CREATE = "create"
    REMOVE = "remove"
    RENAME = "rename"
    WRITE = "write"


@dataclass
class FilesystemEvent:
    """A change in a watched directory."""

    name: str
    """Path relative to the watched directory."""
    type: FilesystemEventType
    entry: Optional[EntryInfo] = None

"""Types of the volume API (same names, fields and values as e2b's).

The e2b SDK is MIT-licensed; this module matches its public API.
"""

import datetime
from dataclasses import dataclass
from enum import Enum
from typing import Optional


class VolumeEntryStatType(str, Enum):
    """The kind of a volume entry."""

    DIRECTORY = "directory"
    FILE = "file"
    SYMLINK = "symlink"
    UNKNOWN = "unknown"

    def __str__(self) -> str:
        return str(self.value)


# Type alias for file type enum
VolumeFileType = VolumeEntryStatType


@dataclass
class VolumeInfo:
    """Information about a volume."""

    volume_id: str
    """Volume ID."""
    name: str
    """Volume name."""


@dataclass
class VolumeAndToken(VolumeInfo):
    """Information about a volume and its auth token."""

    token: str
    """Volume auth token. Cua Volume authenticates through your Cua account, so
    this is the empty string."""
    domain: Optional[str] = None
    """Domain of the volume content host; None on Cua."""


@dataclass
class VolumeEntryStat:
    """Stat of a volume entry."""

    name: str
    """Name of the filesystem object."""
    type: VolumeFileType
    """Type of the filesystem object."""
    path: str
    """Path to the filesystem object."""
    size: int
    """Size of the filesystem object."""
    mode: int
    """Mode of the filesystem object."""
    uid: int
    """User ID of the filesystem object."""
    gid: int
    """Group ID of the filesystem object."""
    atime: datetime.datetime
    """Access time."""
    mtime: datetime.datetime
    """Modification time."""
    ctime: datetime.datetime
    """Creation time."""
    target: Optional[str] = None
    """Target path for symlinks."""


__all__ = [
    "VolumeInfo",
    "VolumeAndToken",
    "VolumeEntryStat",
    "VolumeFileType",
]

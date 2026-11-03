"""Helpers of the volume classes: the instance-or-class method descriptor and
the conversion of a raw entry to `VolumeEntryStat`.

The e2b SDK is MIT-licensed; this module matches its public API.
"""

import datetime
from dataclasses import dataclass
from typing import Optional

from e2b.volume.types import VolumeEntryStat, VolumeEntryStatType


@dataclass
class RawEntryStat:
    """An entry as the volume store reports it (e2b's generated
    `VolumeEntryStat` model: `type_` rather than `type`)."""

    name: str
    type_: VolumeEntryStatType
    path: str
    size: int
    mode: int
    uid: int
    gid: int
    atime: datetime.datetime
    mtime: datetime.datetime
    ctime: datetime.datetime
    target: Optional[str] = None


def _ensure_utc(dt: datetime.datetime) -> datetime.datetime:
    """Marks a timezone-naive datetime as UTC."""
    if dt.tzinfo is None:
        return dt.replace(tzinfo=datetime.timezone.utc)
    return dt


def convert_volume_entry_stat(api_stat: RawEntryStat) -> VolumeEntryStat:
    """A raw entry as the SDK's `VolumeEntryStat`."""
    target: Optional[str] = None
    if api_stat.target is not None:
        target = str(api_stat.target)
    return VolumeEntryStat(
        name=api_stat.name,
        type=api_stat.type_,
        path=api_stat.path,
        size=api_stat.size,
        mode=api_stat.mode,
        uid=api_stat.uid,
        gid=api_stat.gid,
        atime=_ensure_utc(api_stat.atime),
        mtime=_ensure_utc(api_stat.mtime),
        ctime=_ensure_utc(api_stat.ctime),
        target=target,
    )


class DualMethod:
    """One name for a class-level and an instance method: `Volume.get_info(id)`
    looks a volume up, `volume.get_info(path)` stats an entry of it.

    `static_fn` may be a `classmethod`; it is then bound to the class it is
    reached through, so it sees the class's bound API params.
    """

    def __init__(self, static_fn, instance_fn):
        self._static_fn = static_fn
        self._instance_fn = instance_fn

    def __get__(self, obj, objtype=None):
        if obj is None:
            return self._static_fn.__get__(None, objtype)
        return self._instance_fn.__get__(obj, objtype)

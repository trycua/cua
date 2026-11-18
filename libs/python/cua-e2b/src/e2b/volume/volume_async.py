"""e2b volumes on Cua Volume (awaitable).

Cua has one versioned volume per user. An e2b volume `name` is the folder
`e2b-volumes/<name>/` of it (its id is its name), so it shows in Finder and in
every Space that mounts the volume, keeps every version, and syncs to the
user's own storage. The file calls go through the SDK's `volume_*` methods, so
they work from any process without a mount.

Cua Volume is an object store, so directories are kept like this: a directory
exists while it holds files, and `make_dir` leaves a marker object
(`.e2b-dir`) in it so an empty one exists too. `uid`, `gid` and `mode` of an
entry (and the times of a directory) are kept as metadata in the volume's own
marker object (`.e2b-volume.json`) and returned by the stat calls; they are
not enforced on the files, since the folder has no owners. Symbolic links
cannot be stored. A volume has no auth token and no content host (Cua
authenticates through your account): `token` is empty, and `domain`, `debug`,
`proxy`, `http_version` and the other connection options of a call are kept
on the object and not used.

The e2b SDK is MIT-licensed; this module matches its public API.
"""

from __future__ import annotations

import datetime
import json
from typing import IO, AsyncIterator, List, Optional, Union

from e2b._backend import get_backend
from e2b.connection_config import ClientFactory, HttpVersion, ProxyTypes
from e2b.exceptions import (
    InvalidArgumentException,
    NotFoundException,
    VolumeException,
    VolumeNotFoundException,
    VolumePathNotFoundException,
)
from e2b.volume.connection_config import VolumeConnectionConfig
from e2b.volume.types import (
    VolumeAndToken,
    VolumeEntryStat,
    VolumeEntryStatType,
    VolumeInfo,
)
from e2b.volume.utils import DualMethod, RawEntryStat, convert_volume_entry_stat

ROOT = "e2b-volumes"
MARKER = ".e2b-volume.json"
DIR_MARKER = ".e2b-dir"
_HIDDEN = (MARKER, DIR_MARKER)
_EPOCH = datetime.datetime.fromtimestamp(0, tz=datetime.timezone.utc)
_CHUNK = 64 * 1024


def mount_source(name: str) -> str:
    """Where the volume is inside a Space that mounts Cua Volume."""
    return f"/volume/{ROOT}/{name}"


async def _client():
    return (await get_backend().client()).spaces()


def _norm(path: str) -> str:
    parts = [p for p in str(path).split("/") if p not in ("", ".")]
    if ".." in parts:
        raise InvalidArgumentException("a volume path cannot contain '..'")
    return "/".join(parts)


def _ts(ms: Optional[int]) -> datetime.datetime:
    if not ms:
        return _EPOCH
    return datetime.datetime.fromtimestamp(ms / 1000, tz=datetime.timezone.utc)


def _now_ms() -> int:
    return int(datetime.datetime.now(tz=datetime.timezone.utc).timestamp() * 1000)


def _is_missing(e: BaseException) -> bool:
    return type(e).__name__ == "NotFound" or "not found" in str(e).lower()


def _not_found(path: str) -> VolumePathNotFoundException:
    return VolumePathNotFoundException(f"Path {path} not found")


class AsyncVolume(ClientFactory):
    """A named volume: `await AsyncVolume.create("data")`."""

    def __init__(
        self,
        volume_id: str,
        name: str,
        token: Optional[str] = None,
        domain: Optional[str] = None,
        debug: Optional[bool] = None,
        proxy: Optional[ProxyTypes] = None,
        *,
        http_version: Optional[HttpVersion] = None,
    ):
        self._volume_id = volume_id
        self._name = name
        self._token = token
        self._domain = domain
        self._debug = debug
        self._proxy = proxy
        self._http_version = http_version

    @property
    def volume_id(self) -> str:
        return self._volume_id

    @property
    def name(self) -> str:
        return self._name

    @property
    def token(self) -> Optional[str]:
        return self._token

    def _get_volume_config(self, **opts) -> VolumeConnectionConfig:
        return VolumeConnectionConfig(
            domain=opts.get("domain") or self._domain,
            debug=opts.get("debug") if opts.get("debug") is not None else self._debug,
            token=opts.get("token") or self._token,
            api_url=opts.get("api_url"),
            request_timeout=opts.get("request_timeout"),
            headers=opts.get("headers"),
            logger=opts.get("logger"),
            proxy=opts.get("proxy") if opts.get("proxy") is not None else self._proxy,
            http_version=(
                opts.get("http_version")
                if opts.get("http_version") is not None
                else self._http_version
            ),
        )

    def _key(self, path: str = "") -> str:
        p = _norm(path)
        return f"{ROOT}/{self._volume_id}" + (f"/{p}" if p else "")

    # ── volumes ────────────────────────────────────────────────────────────

    @classmethod
    def _from_params(cls, name: str, token: str, params: dict) -> "AsyncVolume":
        return cls(
            volume_id=name,
            name=name,
            token=token,
            domain=params.get("domain"),
            debug=params.get("debug"),
            proxy=params.get("proxy"),
            http_version=params.get("http_version"),
        )

    @classmethod
    async def create(cls, name: str, **opts) -> "AsyncVolume":
        """Creates a volume (a folder of Cua Volume); an existing one of that
        name is returned as it is.

        :param name: name of the volume, one folder name
        """
        params = cls._resolve_api_params(**opts)
        if not name or "/" in name or name.startswith("."):
            raise InvalidArgumentException("a volume name is one folder name")
        s = await _client()
        meta = json.dumps({"volume_id": name, "name": name, "meta": {}}).encode()
        try:
            await s.volume_write(f"{ROOT}/{name}/{MARKER}", meta, None, True, None)
        except Exception as e:  # noqa: BLE001
            if "exist" not in str(e).lower():
                raise VolumeException(str(e)) from e
        return cls._from_params(name, "", params)

    @classmethod
    async def connect(cls, volume_id: str, **opts) -> "AsyncVolume":
        """Connects to an existing volume by its id.

        :raises VolumeNotFoundException: when there is none
        """
        info = await cls.get_info(volume_id, **opts)
        return cls._from_params(info.volume_id, info.token, cls._resolve_api_params(**opts))

    @classmethod
    async def _class_get_info(cls, volume_id: str, **opts) -> VolumeAndToken:
        """Information about a volume.

        :raises VolumeNotFoundException: when there is none
        """
        cls._resolve_api_params(**opts)
        s = await _client()
        try:
            await s.volume_read(f"{ROOT}/{volume_id}/{MARKER}", None, None)
        except Exception as e:  # noqa: BLE001
            if _is_missing(e):
                raise VolumeNotFoundException(f"Volume {volume_id} not found") from e
            raise VolumeException(str(e)) from e
        return VolumeAndToken(volume_id=volume_id, name=volume_id, token="", domain=None)

    @classmethod
    async def _class_list(cls, **opts) -> List[VolumeInfo]:
        """All volumes."""
        cls._resolve_api_params(**opts)
        s = await _client()
        try:
            listing = await s.volume_ls(ROOT, None)
        except Exception as e:  # noqa: BLE001
            if _is_missing(e):
                return []
            raise VolumeException(str(e)) from e
        return [VolumeInfo(volume_id=e.name, name=e.name) for e in listing.entries if e.folder]

    @classmethod
    async def destroy(cls, volume_id: str, **opts) -> bool:
        """Deletes a volume with all its files. `False`: there was none."""
        try:
            await cls._class_get_info(volume_id, **opts)
        except VolumeNotFoundException:
            return False
        await cls(volume_id, volume_id)._remove_tree("")
        return True

    # ── store ──────────────────────────────────────────────────────────────

    async def _entries(self, path: str):
        s = await _client()
        try:
            return (await s.volume_ls(self._key(path), None)).entries
        except Exception as e:  # noqa: BLE001
            if _is_missing(e):
                raise _not_found(path) from e
            raise VolumeException(str(e)) from e

    async def _meta(self) -> dict:
        s = await _client()
        try:
            f = await s.volume_read(self._key(MARKER), None, None)
        except Exception as e:  # noqa: BLE001
            if _is_missing(e):
                raise VolumeNotFoundException(f"Volume {self._volume_id} not found") from e
            raise VolumeException(str(e)) from e
        return json.loads(bytes(f.content) or b"{}")

    async def _save_meta(self, meta: dict) -> None:
        s = await _client()
        await s.volume_write(self._key(MARKER), json.dumps(meta).encode(), None, False, None)

    async def _row(self, path: str):
        """The listing row of `path` (None: the volume root or missing)."""
        p = _norm(path)
        parent, _, leaf = p.rpartition("/")
        try:
            return next((e for e in await self._entries(parent) if e.name == leaf), None)
        except VolumePathNotFoundException:
            return None

    async def _is_dir(self, path: str) -> bool:
        p = _norm(path)
        if not p:
            return True
        row = await self._row(p)
        return bool(row and row.folder)

    def _stat(self, path: str, folder: bool, size: int, mtime_ms: Optional[int], meta: dict):
        p = _norm(path)
        m = (meta.get("meta") or {}).get(p, {})
        when = _ts(mtime_ms if mtime_ms else m.get("mtime_ms"))
        made = _ts(m.get("ctime_ms")) if m.get("ctime_ms") else when
        return convert_volume_entry_stat(
            RawEntryStat(
                name=p.rsplit("/", 1)[-1],
                type_=VolumeEntryStatType.DIRECTORY if folder else VolumeEntryStatType.FILE,
                path="/" + p,
                size=0 if folder else size,
                mode=m.get("mode", 0o755 if folder else 0o644),
                uid=m.get("uid", 0),
                gid=m.get("gid", 0),
                atime=when,
                mtime=when,
                ctime=made,
            )
        )

    async def _stat_of(self, path: str) -> VolumeEntryStat:
        p = _norm(path)
        meta = await self._meta()
        if not p:
            return self._stat("", True, 0, None, meta)
        row = await self._row(p)
        if row is None:
            raise _not_found(path)
        return self._stat(p, bool(row.folder), row.size, row.modified_ms, meta)

    async def _remove_tree(self, path: str) -> None:
        s = await _client()
        for e in await self._entries(path):
            if e.folder:
                await self._remove_tree(_norm(f"{path}/{e.name}"))
            else:
                await s.volume_delete(e.path, None, None)

    async def _record(self, path: str, uid=None, gid=None, mode=None, **times) -> None:
        """Keeps uid/gid/mode (and times) of an entry in the volume's metadata."""
        given = {k: v for k, v in (("uid", uid), ("gid", gid), ("mode", mode)) if v is not None}
        given.update(times)
        if not given:
            return
        meta = await self._meta()
        meta.setdefault("meta", {}).setdefault(_norm(path), {}).update(given)
        await self._save_meta(meta)

    async def _require_parent(self, path: str, force: Optional[bool]) -> None:
        """A write needs its parent directory unless `force` creates it."""
        parent = _norm(path).rpartition("/")[0]
        if force or await self._is_dir(parent):
            return
        raise _not_found(path)

    # ── directories and files ──────────────────────────────────────────────

    async def _instance_list(
        self, path: str, depth: Optional[int] = None, **opts
    ) -> List[VolumeEntryStat]:
        """Lists a directory.

        :param path: path of the directory
        :param depth: how many layers to recurse into (default 1: the directory itself)
        :return: the entries (files and directories) below the path
        """
        self._get_volume_config(**opts)
        levels = 1 if depth is None else depth
        p = _norm(path)
        if p:
            row = await self._row(p)
            if row is None:
                raise _not_found(path)
            if not row.folder:
                raise VolumeException(f"Path {path} is not a directory")
        meta = await self._meta()
        out: List[VolumeEntryStat] = []

        async def walk(prefix: str, remaining: int) -> None:
            for e in await self._entries(prefix):
                if e.name in _HIDDEN:
                    continue
                sub = f"{prefix}/{e.name}" if prefix else e.name
                out.append(self._stat(sub, bool(e.folder), e.size, e.modified_ms, meta))
                if e.folder and remaining > 1:
                    await walk(sub, remaining - 1)

        if levels >= 1:
            await walk(p, levels)
        return out

    async def make_dir(
        self,
        path: str,
        uid: Optional[int] = None,
        gid: Optional[int] = None,
        mode: Optional[int] = None,
        force: Optional[bool] = None,
        **opts,
    ) -> VolumeEntryStat:
        """Creates a directory.

        :param uid: owner user id, kept as metadata
        :param gid: owner group id, kept as metadata
        :param mode: mode, kept as metadata
        :param force: create missing parent directories (without it a missing
            parent raises `VolumePathNotFoundException`)
        :return: the stat of the directory
        """
        self._get_volume_config(**opts)
        p = _norm(path)
        if not p:
            return await self._stat_of("")
        row = await self._row(p)
        if row is not None and not row.folder:
            raise VolumeException(f"Path {path} already exists and is a file")
        if row is None:
            await self._require_parent(path, force)
            s = await _client()
            await s.volume_write(self._key(f"{p}/{DIR_MARKER}"), b"", None, False, None)
            await self._record(p, uid, gid, mode, ctime_ms=_now_ms(), mtime_ms=_now_ms())
        else:
            await self._record(p, uid, gid, mode)
        return await self._stat_of(p)

    async def exists(self, path: str, **opts) -> bool:
        """Whether a file or directory exists at `path`."""
        try:
            await self.get_info(path, **opts)
            return True
        except NotFoundException:
            return False

    async def _instance_get_info(self, path: str, **opts) -> VolumeEntryStat:
        """The stat of a file or directory.

        :raises VolumePathNotFoundException: when there is none
        """
        self._get_volume_config(**opts)
        return await self._stat_of(path)

    get_info = DualMethod(_class_get_info, _instance_get_info)
    list = DualMethod(_class_list, _instance_list)

    async def update_metadata(
        self,
        path: str,
        uid: Optional[int] = None,
        gid: Optional[int] = None,
        mode: Optional[int] = None,
        **opts,
    ) -> VolumeEntryStat:
        """Changes the uid, gid or mode kept for a file or directory.

        :raises VolumePathNotFoundException: when there is none
        """
        self._get_volume_config(**opts)
        p = _norm(path)
        if p and await self._row(p) is None:
            raise _not_found(path)
        await self._record(p, uid, gid, mode)
        return await self._stat_of(p)

    async def read_file(
        self,
        path: str,
        format: str = "text",
        stream_idle_timeout: Optional[float] = None,
        **opts,
    ) -> Union[str, bytes, AsyncIterator[bytes]]:
        """Reads a file.

        :param format: `"text"` (default), `"bytes"` or `"stream"` (an async
            iterator of chunks; errors surface when it is first iterated)
        :param stream_idle_timeout: accepted; chunks of a stream are read from
            memory, so none can stall
        :raises VolumePathNotFoundException: when there is none
        """
        self._get_volume_config(**opts)

        async def fetch() -> bytes:
            if await self._is_dir(path):
                raise VolumeException(f"Path {path} is a directory")
            s = await _client()
            try:
                f = await s.volume_read(self._key(path), None, None)
            except Exception as e:  # noqa: BLE001
                if _is_missing(e):
                    raise _not_found(path) from e
                raise VolumeException(str(e)) from e
            return bytes(f.content)

        if format == "stream":

            async def chunks() -> AsyncIterator[bytes]:
                data = await fetch()
                for i in range(0, len(data), _CHUNK):
                    yield data[i : i + _CHUNK]

            return chunks()
        data = await fetch()
        return data if format == "bytes" else data.decode("utf-8", errors="replace")

    async def write_file(
        self,
        path: str,
        data: Union[str, bytes, IO],
        uid: Optional[int] = None,
        gid: Optional[int] = None,
        mode: Optional[int] = None,
        force: Optional[bool] = None,
        **opts,
    ) -> VolumeEntryStat:
        """Writes a file, creating it or overwriting it.

        :param data: a string, bytes or a file-like object (read to the end)
        :param uid: owner user id, kept as metadata
        :param gid: owner group id, kept as metadata
        :param mode: mode, kept as metadata
        :param force: create missing parent directories (without it a missing
            parent raises `VolumePathNotFoundException`)
        :return: the stat of the file
        """
        self._get_volume_config(**opts)
        p = _norm(path)
        if not p or p.rsplit("/", 1)[-1] in _HIDDEN:
            raise InvalidArgumentException(f"{path!r} is not a file path")
        if isinstance(data, str):
            raw = data.encode("utf-8")
        elif isinstance(data, bytes):
            raw = data
        elif hasattr(data, "read"):
            raw = data.read()
            raw = raw.encode("utf-8") if isinstance(raw, str) else bytes(raw)
        else:
            raise ValueError(f"Unsupported data type: {type(data)}")
        await self._require_parent(path, force)
        if await self._is_dir(p):
            raise VolumeException(f"Path {path} is a directory")
        # A file cannot sit below another file.
        parts = p.split("/")
        for i in range(1, len(parts)):
            above = await self._row("/".join(parts[:i]))
            if above is not None and not above.folder:
                raise VolumeException(f"Path /{'/'.join(parts[:i])} is a file, not a directory")
        s = await _client()
        obj = await s.volume_write(self._key(p), raw, None, False, None)
        await self._record(p, uid, gid, mode)
        return self._stat(p, False, obj.size, obj.modified_ms, await self._meta())

    async def remove(self, path: str, **opts) -> None:
        """Removes a file, or a directory with everything in it.

        :raises VolumePathNotFoundException: when there is none
        """
        self._get_volume_config(**opts)
        p = _norm(path)
        if not p:
            raise VolumeException("Cannot remove the root of a volume; use destroy")
        row = await self._row(p)
        if row is None:
            raise _not_found(path)
        if row.folder:
            await self._remove_tree(p)
        else:
            await (await _client()).volume_delete(row.path, None, None)
        meta = await self._meta()
        gone = [k for k in meta.get("meta", {}) if k == p or k.startswith(p + "/")]
        for k in gone:
            meta["meta"].pop(k)
        if gone:
            await self._save_meta(meta)


__all__ = ["AsyncVolume", "mount_source"]

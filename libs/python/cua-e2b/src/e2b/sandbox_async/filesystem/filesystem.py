"""`sandbox.files`: e2b's Filesystem over cua-spacesd's file service."""

from __future__ import annotations

import asyncio
import io
import os
import posixpath
import stat as _stat
import tempfile
from datetime import datetime, timezone
from typing import IO, Dict, List, Literal, Optional, Union, overload

from e2b._errors import map_error
from e2b.exceptions import (
    AuthenticationException,
    FileNotFoundException,
    InvalidArgumentException,
    SandboxException,
)
from e2b.sandbox.filesystem.filesystem import (
    AsyncFileStreamReader,
    EntryInfo,
    FileType,
    WriteEntry,
    WriteInfo,
    validate_metadata,
)
from e2b.sandbox.filesystem.watch_handle import FilesystemEvent
from e2b.sandbox_async.commands.command import _bounded, guest_user
from e2b.sandbox_async.filesystem.watch_handle import AsyncWatchHandle, watch_open_error
from e2b.sandbox_async.utils import OutputHandler

Username = str
_TYPES = {"file": FileType.FILE, "dir": FileType.DIR, "symlink": FileType.SYMLINK}
_EPOCH = datetime.fromtimestamp(0, tz=timezone.utc)


_TYPE_BITS = {"file": _stat.S_IFREG, "dir": _stat.S_IFDIR, "symlink": _stat.S_IFLNK}

# envd's own limits on file metadata (the keys are stored as `user.e2b.<key>`).
_META_KEY_MAX = 246
_META_TOTAL_MAX = 4096
_META_PREFIX = len("user.e2b.")


def _entry(s) -> EntryInfo:
    return EntryInfo(
        name=s.name,
        type=_TYPES.get(s.type),
        path=s.path,
        size=s.size,
        mode=s.mode,
        permissions=_stat.filemode(_TYPE_BITS.get(s.type, 0) | (s.mode & 0o7777)),
        owner=getattr(s, "owner", ""),
        group=getattr(s, "group", ""),
        modified_time=s.modified or _EPOCH,
        symlink_target=s.symlink_target,
    )


_CHUNK = 1024 * 1024
_MAX_PARALLEL_UPLOADS = 16


def _is_streamed(data) -> bool:
    return not isinstance(data, (str, bytes))


def _spool(data, path: str):
    """File-like `data` copied in chunks to a local temporary file (memory
    stays bounded whatever its size); returns the file's name. Text-mode
    objects are encoded as UTF-8, as e2b's streamed upload does."""
    if not isinstance(data, io.IOBase):
        raise InvalidArgumentException(f"Unsupported data type: {type(data)}")
    fd, name = tempfile.mkstemp(prefix="cua-e2b-")
    try:
        with os.fdopen(fd, "wb") as out:
            while True:
                chunk = data.read(_CHUNK)
                if not chunk:
                    break
                out.write(chunk.encode("utf-8") if isinstance(chunk, str) else chunk)
    except BaseException:
        os.unlink(name)
        raise
    return name


def _entry_path_data(f):
    if isinstance(f, dict):
        return f["path"], f["data"]
    return f.path, f.data


def _is(e: BaseException, name: str) -> bool:
    return type(e).__name__ == name


def _missing(e: BaseException) -> bool:
    """A path that is not there (the file service's NotFound, or the same
    reported by a command run as root)."""
    return _is(e, "NotFound") or isinstance(e, FileNotFoundError)


def _not_a_dir(e: BaseException) -> bool:
    """A path with a file where a directory should be."""
    return "not a directory" in str(e).lower()


def _validate_stored_metadata(stored: Dict[str, str]) -> None:
    """envd's limits (after the client's own key and value checks)."""
    total = 0
    for k, v in stored.items():
        if len(k.encode()) > _META_KEY_MAX:
            raise InvalidArgumentException(
                f'invalid metadata: metadata key "{k}" exceeds {_META_KEY_MAX} bytes'
            )
        total += _META_PREFIX + len(k.encode()) + len(v.encode())
    if total > _META_TOTAL_MAX:
        raise InvalidArgumentException(
            f"invalid metadata: total metadata size {total} exceeds {_META_TOTAL_MAX} bytes"
        )


class Filesystem:
    """Reads and writes files in the sandbox.

    `home_of(user)` gives a user's home (relative paths resolve against it).
    Every call acts as `user` (default: the sandbox's user), as e2b's do: a
    file written for `root` belongs to root.
    """

    def __init__(self, machine, home_of):
        self._m = machine
        self._home_of = home_of

    async def _is_dir(self, path: str, user: Optional[Username]) -> bool:
        try:
            return (await self._m.stat(path, guest_user(user), follow=True)).type == "dir"
        except Exception:  # noqa: BLE001
            return False

    async def _check_user(self, user: Optional[Username]) -> None:
        """e2b looks the user up for every call."""
        name = guest_user(user)
        if name and not await self._m.has_user(name):
            raise AuthenticationException(
                f"error looking up user '{name}': user: unknown user {name}"
            )

    async def _abs(self, path: str, user: Optional[Username]) -> str:
        if posixpath.isabs(path):
            return path
        if path == "~" or path.startswith("~/"):
            return posixpath.normpath(posixpath.join(await self._home_of(user), path[2:]))
        return posixpath.join(await self._home_of(user), path)

    @overload
    async def read(
        self,
        path: str,
        format: Literal["text"] = "text",
        user: Optional[Username] = None,
        request_timeout: Optional[float] = None,
        gzip: bool = False,
    ) -> str: ...

    @overload
    async def read(
        self,
        path: str,
        format: Literal["bytes"],
        user: Optional[Username] = None,
        request_timeout: Optional[float] = None,
        gzip: bool = False,
    ) -> bytearray: ...

    @overload
    async def read(
        self,
        path: str,
        format: Literal["stream"],
        user: Optional[Username] = None,
        request_timeout: Optional[float] = None,
        gzip: bool = False,
        stream_idle_timeout: Optional[float] = None,
    ) -> AsyncFileStreamReader: ...

    async def read(
        self,
        path: str,
        format: Literal["text", "bytes", "stream"] = "text",
        user: Optional[Username] = None,
        request_timeout: Optional[float] = None,
        gzip: bool = False,
        stream_idle_timeout: Optional[float] = None,
    ):
        """Reads a file as `str`, `bytearray`, or an async stream of chunks."""
        try:
            await self._check_user(user)
            path = await self._abs(path, user)
            if format == "stream":
                # Fail now (as e2b does) for a missing file or a directory,
                # not on the first chunk.
                st = await _bounded(
                    self._m.stat(path, guest_user(user), follow=True), request_timeout
                )
                if st.type == "dir":
                    raise InvalidArgumentException(f"path '{path}' is a directory")
                download = await _bounded(
                    self._m.open_download(path, guest_user(user)), request_timeout
                )
                return AsyncFileStreamReader(download, idle_timeout=stream_idle_timeout or None)
            data = await _bounded(self._m.download(path, guest_user(user)), request_timeout)
        except Exception as e:  # noqa: BLE001
            raise await self._read_error(e, path, user)
        if format == "bytes":
            return bytearray(data)
        if format == "text":
            return data.decode("utf-8", errors="replace")
        return None  # as e2b: any other format is read and then dropped

    async def _read_error(self, e: BaseException, path: str, user) -> BaseException:
        if isinstance(e, (SandboxException, AuthenticationException)):
            return e
        if _missing(e):
            return FileNotFoundException(f"path '{path}' does not exist")
        if await self._is_dir(path, user):
            return InvalidArgumentException(f"path '{path}' is a directory")
        return map_error(e, file=True)

    async def write(
        self,
        path: str,
        data: Union[str, bytes, IO],
        user: Optional[Username] = None,
        request_timeout: Optional[float] = None,
        gzip: bool = False,
        use_octet_stream: Optional[bool] = None,
        metadata: Optional[Dict[str, str]] = None,
    ) -> WriteInfo:
        """Writes a file (parents created, existing files overwritten), owned
        by `user` (default: the sandbox's user). File-like `data` is streamed
        in chunks, not held in memory. `gzip` and `use_octet_stream` choose
        how e2b's HTTP upload is encoded; the transfer to cua-spacesd has its
        own chunked, checksummed framing, so they change nothing here."""
        result = await self.write_files(
            [WriteEntry(path=path, data=data)],
            user=user,
            request_timeout=request_timeout,
            gzip=gzip,
            use_octet_stream=use_octet_stream,
            metadata=metadata,
        )
        if len(result) != 1:
            raise SandboxException("Received unexpected response from write operation")
        return result[0]

    async def write_files(
        self,
        files: List[WriteEntry],
        user: Optional[Username] = None,
        request_timeout: Optional[float] = None,
        gzip: bool = False,
        use_octet_stream: Optional[bool] = None,
        metadata: Optional[Dict[str, str]] = None,
    ) -> List[WriteInfo]:
        """Writes several files concurrently; the results are in order."""
        if len(files) == 0:
            return []
        validate_metadata(metadata)
        entries = [_entry_path_data(f) for f in files]
        octet = (
            use_octet_stream
            if use_octet_stream is not None
            else any(_is_streamed(d) for _, d in entries)
        )
        for p, d in entries:
            if isinstance(d, (str, bytes)):
                continue
            if not isinstance(d, io.IOBase):
                # e2b: the octet-stream path says "data type", multipart names the file
                if octet or gzip:
                    raise InvalidArgumentException(f"Unsupported data type: {type(d)}")
                raise InvalidArgumentException(f"Unsupported data type for file {p}")
        stored = {k.lower(): v for k, v in metadata.items()} if metadata else None
        if stored:
            _validate_stored_metadata(stored)
        try:
            await self._check_user(user)
            paths = [await self._abs(p, user) for p, _ in entries]
        except Exception as e:  # noqa: BLE001
            raise map_error(e, file=True)
        # A multipart upload (not octet-stream) is refused by envd at the
        # first path it has already written, after writing the ones before.
        duplicate = None
        if not (octet or gzip):
            seen = set()
            for i, p in enumerate(paths):
                if p in seen:
                    duplicate = (i, p)
                    break
                seen.add(p)
        if duplicate is not None:
            entries, paths = entries[: duplicate[0]], paths[: duplicate[0]]
        gate = asyncio.Semaphore(_MAX_PARALLEL_UPLOADS)
        # an agent with `fs.xattr` stores the metadata as part of the write
        meta = stored if stored and await self._m.xattr_native() else None

        async def one(p: str, d) -> WriteInfo:
            async with gate:
                try:
                    if isinstance(d, (str, bytes)):
                        raw = d.encode() if isinstance(d, str) else d
                        await _bounded(
                            self._m.upload(p, raw, guest_user(user), metadata=meta),
                            request_timeout,
                        )
                    else:
                        tmp = await asyncio.to_thread(_spool, d, p)
                        try:
                            await _bounded(
                                self._m.upload(
                                    p, b"", guest_user(user), local_path=tmp, metadata=meta
                                ),
                                0 if _is_streamed(d) else request_timeout,
                            )
                        finally:
                            os.unlink(tmp)
                    if stored and meta is None:
                        await self._set_metadata(p, stored)
                except Exception as e:  # noqa: BLE001
                    raise await self._write_error(e, p, user)
                return WriteInfo(
                    name=posixpath.basename(p), type=FileType.FILE, path=p, metadata=stored
                )

        results = await asyncio.gather(
            *(one(p, d) for p, (_, d) in zip(paths, entries)), return_exceptions=True
        )
        for r in results:
            if isinstance(r, BaseException):
                raise r
        if duplicate is not None:
            raise InvalidArgumentException(
                f"you cannot upload multiple files to the same path '{duplicate[1]}' in one "
                "upload request, only the first specified file was uploaded"
            )
        return list(results)

    async def _write_error(self, e: BaseException, path: str, user) -> BaseException:
        """The e2b exception for a write to `path` that failed with `e`."""
        if isinstance(e, (SandboxException, AuthenticationException)):
            return e
        if await self._is_dir(path, user):
            return InvalidArgumentException(f"path is a directory: {path}")
        if _not_a_dir(e):
            parent = posixpath.dirname(path)
            while parent not in ("", "/"):
                try:
                    if (await self._m.stat(parent, guest_user(user), follow=True)).type != "dir":
                        break
                except Exception:  # noqa: BLE001
                    pass
                parent = posixpath.dirname(parent)
            return SandboxException(f"500: error ensuring directories: path is a file: {parent}")
        if _missing(e):
            return SandboxException(
                f"500: error opening file: open {path}: no such file or directory"
            )
        return map_error(e, file=True)

    async def _py(self, script: str, *args: str) -> str:
        """Runs a short python3 script in the guest (as root) and returns
        its stdout."""
        proc = await self._m.spawn(
            ["python3", "-c", script, *args],
            env={},
            cwd=None,
            user="root",
            timeout_ms=30_000,
            stdin=False,
        )
        out = b""
        while (ev := await proc.next_event()) is not None and ev.kind != "exit":
            if ev.kind == "stdout":
                out += ev.data
        if ev is None or ev.exit_code != 0:
            raise SandboxException(
                f"python3 in the sandbox failed (exit {getattr(ev, 'exit_code', None)})"
            )
        return out.decode()

    async def _set_metadata(self, path: str, metadata: Dict[str, str]) -> None:
        """e2b keeps metadata as `user.e2b.*` extended attributes."""
        import json

        await self._py(
            "import json,os,sys\n"
            "for k,v in json.loads(sys.argv[2]).items():\n"
            " os.setxattr(sys.argv[1],'user.e2b.'+k.lower(),v.encode())",
            path,
            json.dumps(metadata),
        )

    async def _metadata(self, path: str) -> Optional[Dict[str, str]]:
        import json

        if await self._m.xattr_native():
            return (await self._m.stat(path, None, follow=True)).metadata or None

        out = await self._py(
            "import json,os,sys\n"
            "p=sys.argv[1]\n"
            "try: names=os.listxattr(p)\n"
            "except OSError: names=[]\n"
            "print(json.dumps({n[9:]:os.getxattr(p,n).decode('utf-8','replace') "
            "for n in names if n.startswith('user.e2b.')}))",
            path,
        )
        return json.loads(out) or None

    async def _metadata_of(self, paths: List[str]) -> Dict[str, Dict[str, str]]:
        """The `user.e2b.*` attributes of each of `paths` that has any."""
        import json

        out: Dict[str, Dict[str, str]] = {}
        for i in range(0, len(paths), 200):
            out.update(
                json.loads(
                    await self._py(
                        "import json,os,sys\n"
                        "r={}\n"
                        "for p in sys.argv[1:]:\n"
                        " try:\n"
                        "  m={n[9:]:os.getxattr(p,n,follow_symlinks=False).decode('utf-8','replace')"
                        " for n in os.listxattr(p,follow_symlinks=False) if n.startswith('user.e2b.')}\n"
                        " except OSError: continue\n"
                        " if m: r[p]=m\n"
                        "print(json.dumps(r))",
                        *paths[i : i + 200],
                    )
                )
            )
        return out

    async def list(
        self,
        path: str,
        depth: Optional[int] = 1,
        user: Optional[Username] = None,
        request_timeout: Optional[float] = None,
    ) -> List[EntryInfo]:
        if depth is not None and depth < 1:
            raise InvalidArgumentException("depth should be at least 1")
        try:
            await self._check_user(user)
            path = await self._abs(path, user)
            entries = await _bounded(
                self._m.list_dir(path, depth or 1, guest_user(user)), request_timeout
            )
        except Exception as e:  # noqa: BLE001
            if isinstance(e, (SandboxException, AuthenticationException)):
                raise
            if _missing(e):
                raise FileNotFoundException(
                    f"path not found: lstat {path}: no such file or directory"
                ) from e
            try:
                st = await self._m.stat(path, guest_user(user), follow=True)
                if st.type != "dir":
                    raise InvalidArgumentException(f"path is not a directory: {path}") from e
            except InvalidArgumentException:
                raise
            except Exception:  # noqa: BLE001
                pass
            raise map_error(e, file=True)
        try:
            if entries and await self._m.xattr_native():
                found = {s.path: s.metadata for s in entries if s.metadata}
            else:
                found = await self._metadata_of([s.path for s in entries])
        except Exception as e:  # noqa: BLE001
            raise map_error(e, file=True)
        out = [_entry(s) for s in entries]
        for e in out:
            e.metadata = found.get(e.path)
        return out

    async def exists(
        self, path: str, user: Optional[Username] = None, request_timeout: Optional[float] = None
    ) -> bool:
        try:
            await self._check_user(user)
            path = await self._abs(path, user)
            await _bounded(self._m.stat(path, guest_user(user)), request_timeout)
            return True
        except Exception as e:  # noqa: BLE001
            if _missing(e):
                return False
            raise self._info_error(e, path)

    @staticmethod
    def _info_error(e: BaseException, path: str) -> BaseException:
        if _not_a_dir(e) and not isinstance(e, SandboxException):
            return SandboxException(
                f"Code.INTERNAL: error getting file info: lstat {path}: not a directory"
            )
        return map_error(e, file=True)

    async def get_info(
        self, path: str, user: Optional[Username] = None, request_timeout: Optional[float] = None
    ) -> EntryInfo:
        try:
            await self._check_user(user)
            path = await self._abs(path, user)
            entry = _entry(await _bounded(self._m.stat(path, guest_user(user)), request_timeout))
            entry.metadata = await self._metadata(path)
            return entry
        except Exception as e:  # noqa: BLE001
            if _missing(e):
                raise FileNotFoundException(
                    f"file not found: lstat {path}: no such file or directory"
                ) from e
            raise self._info_error(e, path)

    async def remove(
        self, path: str, user: Optional[Username] = None, request_timeout: Optional[float] = None
    ) -> None:
        """Removes a file or a directory (recursively). A missing path is not
        an error (as with e2b)."""
        try:
            await self._check_user(user)
            path = await self._abs(path, user)
            await _bounded(self._m.remove(path, guest_user(user)), request_timeout)
        except Exception as e:  # noqa: BLE001
            if _missing(e):
                return
            if isinstance(e, PermissionError):
                raise SandboxException(
                    f"Code.INTERNAL: error removing file or directory: {e}"
                ) from e
            raise map_error(e, file=True)

    async def rename(
        self,
        old_path: str,
        new_path: str,
        user: Optional[Username] = None,
        request_timeout: Optional[float] = None,
    ) -> EntryInfo:
        """Renames as Go's `os.Rename` (what e2b calls) does: parents of the
        destination are created, an existing file is replaced by a file, and
        an existing directory is never replaced."""
        try:
            await self._check_user(user)
            old_path = await self._abs(old_path, user)
            new_path = await self._abs(new_path, user)
            gu = guest_user(user)
            try:
                dst = await _bounded(self._m.stat(new_path, gu), request_timeout)
            except Exception as e:  # noqa: BLE001
                if not _missing(e):
                    raise
                dst = None
            if dst is not None:
                src = await _bounded(self._m.stat(old_path, gu), request_timeout)
                err = None
                if dst.type == "dir":
                    err = "file exists"
                elif src.type == "dir":
                    err = "not a directory"
                if err:
                    raise SandboxException(
                        f"Code.INTERNAL: error renaming: rename {old_path} {new_path}: {err}"
                    )
                if old_path == new_path:
                    entry = _entry(dst)
                    entry.metadata = await self._metadata(new_path)
                    return entry
            entry = _entry(await _bounded(self._m.rename(old_path, new_path, gu), request_timeout))
            entry.metadata = await self._metadata(new_path)
            return entry
        except Exception as e:  # noqa: BLE001
            if _missing(e):
                raise FileNotFoundException(
                    f"source file not found: rename {old_path} {new_path}: "
                    "no such file or directory"
                ) from e
            raise map_error(e, file=True)

    async def make_dir(
        self, path: str, user: Optional[Username] = None, request_timeout: Optional[float] = None
    ) -> bool:
        """Creates a directory and its parents. `False` if it already existed."""
        abs_path = path
        try:
            await self._check_user(user)
            abs_path = await self._abs(path, user)
            st = await self._m.stat(abs_path, guest_user(user), follow=True)
        except AuthenticationException:
            raise
        except Exception as e:  # noqa: BLE001 - nothing there yet (or not readable: create decides)
            st = None
            if _not_a_dir(e):
                raise self._dir_error(abs_path) from e
        if st is not None:
            if st.type == "dir":
                return False
            raise InvalidArgumentException(
                f"path already exists but it is not a directory: {abs_path}"
            )
        try:
            path = await self._abs(path, user)
            await _bounded(self._m.make_dir(path, guest_user(user)), request_timeout)
        except Exception as e:  # noqa: BLE001
            if _not_a_dir(e) and not isinstance(e, SandboxException):
                raise self._dir_error(path) from e
            raise map_error(e, file=True)
        return True

    @staticmethod
    def _dir_error(path: str) -> BaseException:
        return SandboxException(
            f"Code.INTERNAL: error getting file info: stat {path}: not a directory"
        )

    async def watch_dir(
        self,
        path: str,
        on_event: OutputHandler[FilesystemEvent],
        on_exit: Optional[OutputHandler[Optional[Exception]]] = None,
        user: Optional[Username] = None,
        request_timeout: Optional[float] = None,
        timeout: Optional[float] = 60,
        recursive: bool = False,
        include_entry: bool = False,
        allow_network_mounts: bool = False,
    ) -> AsyncWatchHandle:
        """Watches a directory; `on_event` gets each change (`name` relative
        to `path`). `timeout` seconds (0: none) ends the watch."""
        try:
            path = await self._abs(path, user)
            watcher = await _bounded(
                self._m.watch(path, recursive, guest_user(user)), request_timeout
            )
        except Exception as e:  # noqa: BLE001
            raise watch_open_error(e, path)

        async def entry_of(p: str):
            try:
                return _entry(await self._m.stat(p, guest_user(user)))
            except Exception:  # noqa: BLE001 - gone again
                return None

        return AsyncWatchHandle(
            self._m,
            watcher,
            path,
            on_event,
            on_exit,
            timeout or None,
            entry_of=entry_of if include_entry else None,
            opener=asyncio.current_task(),
        )

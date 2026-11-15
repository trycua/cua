"""Where sandboxes come from: the cua SDK (default), or a test double.

The e2b classes talk to a small `Backend` / `Machine` / `Process` surface.
`CuaBackend` implements it with the cua SDK: Spaces of kind `vm` placed with
`on` (default `auto`), checkpoints and forks, cua-spacesd for exec, files
and TCP forwards, and Space lifetimes kept by the cua daemon. Nothing here
speaks a wire protocol of its own: typed SDK calls, plus the SDK's
`call_json` for cua-spacesd RPCs that have no typed wrapper (Stat with
owners, Move, directory watchers).

Configuration (environment):

- ``CUA_E2B_ON``: where new sandboxes go: `auto` (default: the least-loaded
  of your signed-in machines), `local`, `host:<machine>`, `cloud` (Cua
  Cloud), or one of your own clouds (`aws`, `gcp`, `modal`, ... as
  connected with `cua cloud add`). Forks from a checkpoint run where the checkpoint is.
  `direct:<url>` uses an existing machine running cua-spacesd (with
  ``CUA_E2B_TOKEN``) as every sandbox; `kill` then only forgets it.
- ``CUA_E2B_KIND``: Space kind (default `vm`, Linux on QEMU / KVM).
- ``CUA_E2B_IMAGE``: image for `template=None` / `"base"` (default: the
  canonical Linux image).
- ``CUA_E2B_CPUS`` / ``CUA_E2B_MEMORY_MB`` / ``CUA_E2B_DISK_GB``: VM size.
- ``CUA_E2B_TEMPLATES``: JSON object, or a path to a JSON file, mapping e2b
  template or snapshot ids to Cua checkpoint names or images.
- ``CUA_E2B_MODE``: `auto` (default: the cua daemon, started when none
  runs), `daemon` or `embedded`. ``CUA_E2B_DAEMON``: daemon address.
- ``CUA_E2B_BIND``: where `get_host` forwards listen (default
  `127.0.0.1:0`; `0.0.0.0:0` serves the LAN).
"""

from __future__ import annotations

import asyncio
import concurrent.futures
import contextlib
import contextvars
import inspect
import json
import os
import posixpath
import secrets
import subprocess
import threading
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Dict, List, Optional, Protocol, Tuple


@dataclass
class ProcEvent:
    """One output chunk or the exit of a guest process."""

    kind: str  # "stdout" | "stderr" | "pty" | "exit"
    data: bytes = b""
    exit_code: Optional[int] = None
    signal: Optional[str] = None
    signal_name: Optional[str] = None
    signal_number: Optional[int] = None
    timed_out: bool = False
    error: Optional[str] = None


@dataclass
class FileStat:
    name: str
    path: str
    type: str  # "file" | "dir" | "symlink" | "other"
    size: int
    mode: int
    modified: Optional[datetime]
    symlink_target: Optional[str]
    owner: str = ""
    group: str = ""
    metadata: Optional[Dict[str, str]] = None
    """The `user.e2b.*` extended attributes, when the machine read them with
    the stat (`xattr_native`); `None`: not read."""


@dataclass
class FsEvent:
    path: str
    type: str  # "create" | "write" | "remove" | "rename" | "chmod"
    old_path: str = ""  # a rename seen whole: where the entry was


@dataclass
class MachineInfo:
    """What `Sandbox.get_info` / `list` report."""

    sandbox_id: str
    template: Optional[str]
    started_at: Optional[datetime]
    cpus: int = 0
    memory_mb: int = 0
    state: str = "running"
    extra: dict = field(default_factory=dict)


class Process(Protocol):
    pid: int

    async def next_event(self) -> Optional[ProcEvent]: ...
    async def kill(self) -> bool: ...
    async def write_stdin(self, data: bytes) -> None: ...
    async def write_pty(self, data: bytes) -> None: ...
    async def resize(self, cols: int, rows: int) -> None: ...
    async def close_stdin(self) -> None: ...
    async def detach(self) -> None: ...


class Download(Protocol):
    async def next_chunk(self) -> Optional[bytes]: ...
    def cancel(self) -> None: ...


class Machine(Protocol):
    """One sandbox."""

    sandbox_id: str

    async def spawn(
        self,
        argv: List[str],
        *,
        env: Dict[str, str],
        cwd: Optional[str],
        user: Optional[str],
        timeout_ms: Optional[int],
        stdin: bool,
        pty: Optional[Tuple[int, int]] = None,
    ) -> Process: ...
    async def attach(self, pid: int) -> Process: ...
    async def list_processes(self) -> List[dict]: ...

    # File calls take the guest user they act for (`None`: the sandbox's
    # default user), as e2b's do.
    async def upload(
        self,
        path: str,
        data: bytes,
        user: Optional[str] = None,
        *,
        local_path: Optional[str] = None,
        metadata: Optional[Dict[str, str]] = None,
    ) -> None:
        """Writes `data` (or the local file `local_path`, streamed in chunks:
        memory stays bounded) to `path` as `user`. `metadata` is stored as
        `user.e2b.*` extended attributes, only on a machine whose
        `xattr_native()` is true (otherwise the caller sets it afterwards)."""
        ...

    async def xattr_native(self) -> bool:
        """Whether the agent stores and reports file metadata itself
        (`fs.xattr`): `upload(metadata=)` works, and `stat` / `list_dir` fill
        `FileStat.metadata`."""
        ...

    async def download(self, path: str, user: Optional[str] = None) -> bytes: ...
    async def open_download(self, path: str, user: Optional[str] = None) -> Download: ...
    async def remove(self, path: str, user: Optional[str] = None) -> None: ...
    async def stat(
        self, path: str, user: Optional[str] = None, *, follow: bool = False
    ) -> FileStat: ...
    async def has_user(self, name: str) -> bool: ...
    async def list_dir(
        self, path: str, depth: int, user: Optional[str] = None
    ) -> List[FileStat]: ...
    async def make_dir(self, path: str, user: Optional[str] = None) -> None: ...
    async def rename(self, old: str, new: str, user: Optional[str] = None) -> FileStat: ...
    async def watch(self, path: str, recursive: bool, user: Optional[str] = None) -> str: ...
    async def watch_events(self, watcher_id: str) -> List[FsEvent]: ...
    async def unwatch(self, watcher_id: str) -> None: ...
    async def set_egress(self, allow: List[str], deny: List[str]) -> None: ...
    def endpoint(self) -> Optional[str]: ...
    async def signed_url(self, path: str, method: str, ttl: int, user: Optional[str]) -> str: ...
    async def metrics(self) -> dict: ...
    async def forward(self, port: int) -> str: ...
    async def set_lifetime(self, seconds: float, on_expire: str) -> bool: ...
    async def checkpoint(self, name: str, cold: bool = False) -> str: ...
    async def suspend(self, cold: bool = False) -> None: ...
    async def resume(self, reboot: bool = False) -> None: ...
    async def reboot(self) -> None: ...
    async def fork(self, name: str) -> "Machine": ...
    async def delete(self) -> bool: ...


class Backend(Protocol):
    async def create(
        self,
        template: Optional[str],
        *,
        network: Optional[str] = None,
        egress: Optional[dict] = None,
    ) -> Machine: ...
    async def connect(self, sandbox_id: str, reboot: bool = False) -> Machine: ...
    async def save_template(self, name: str, record: dict) -> None: ...
    def template(self, name: str) -> Optional[dict]: ...
    async def delete_template(self, name: str) -> bool: ...
    async def list_snapshots(self) -> List[dict]: ...
    async def delete_snapshot(self, name: str) -> bool: ...
    async def kill(self, sandbox_id: str) -> bool: ...
    async def list(self) -> List[MachineInfo]: ...
    async def state(self, sandbox_id: str) -> Optional[str]: ...
    def record(self, sandbox_id: str) -> dict: ...
    def annotate(self, sandbox_id: str, **fields) -> None: ...
    def forget(self, sandbox_id: str) -> None: ...


_backend: Optional[Backend] = None


def set_backend(backend: Optional[Backend]) -> None:
    """Replaces the backend (tests); `None` restores the cua SDK."""
    global _backend
    _backend = backend


def get_backend() -> Backend:
    global _backend
    if _backend is None:
        _backend = CuaBackend()
    return _backend


def resolve_template(template: Optional[str]) -> Optional[str]:
    """Applies ``CUA_E2B_TEMPLATES``; `None`/"base" mean the default image."""
    raw = os.environ.get("CUA_E2B_TEMPLATES", "").strip()
    if raw and template is not None:
        if not raw.startswith("{"):
            with open(os.path.expanduser(raw)) as f:
                raw = f.read()
        mapping = json.loads(raw)
        template = mapping.get(template, template)
    if template in (None, "", "base"):
        return os.environ.get("CUA_E2B_IMAGE") or None
    return template


# ── cua SDK backend ────────────────────────────────────────────────────────


_SIZE: contextvars.ContextVar = contextvars.ContextVar("cua_e2b_vm_size", default=None)


@contextlib.contextmanager
def vm_size(cpus: Optional[int] = None, memory_mb: Optional[int] = None):
    """Sizes the VMs created inside the block (this task only; the Template
    builder uses it for its builder VM). Unset values fall back to
    ``CUA_E2B_CPUS`` / ``CUA_E2B_MEMORY_MB``."""
    token = _SIZE.set({"cpus": cpus, "memory_mb": memory_mb})
    try:
        yield
    finally:
        _SIZE.reset(token)


def _size(name: str, key: str) -> Optional[int]:
    chosen = (_SIZE.get() or {}).get(key)
    return chosen if chosen is not None else _env_int(name)


def _env_int(name: str) -> Optional[int]:
    v = os.environ.get(name, "").strip()
    return int(v) if v else None


_KIND = {"STDOUT": "stdout", "STDERR": "stderr", "PTY": "pty", "EXIT": "exit"}
_FILE_KIND = {
    "file": "file",
    "directory": "dir",
    "symlink": "symlink",
    "FILE_TYPE_FILE": "file",
    "FILE_TYPE_DIRECTORY": "dir",
    "FILE_TYPE_SYMLINK": "symlink",
}
_FS_EVENT = {
    "FS_EVENT_TYPE_CREATE": "create",
    "FS_EVENT_TYPE_WRITE": "write",
    "FS_EVENT_TYPE_REMOVE": "remove",
    "FS_EVENT_TYPE_RENAME": "rename",
    "FS_EVENT_TYPE_CHMOD": "chmod",
}
_FS = "/cua.env.v1.FilesystemService/"


def _get(d: dict, *names, default=None):
    for n in names:
        if n in d:
            return d[n]
    return default


def _ts(v) -> Optional[datetime]:
    if not v:
        return None
    try:
        return datetime.fromisoformat(str(v).replace("Z", "+00:00"))
    except ValueError:
        return None


def _stat_json(e: dict) -> FileStat:
    """An `EntryInfo` as proto3 JSON (camelCase or snake_case)."""
    return FileStat(
        name=e.get("name", ""),
        path=e.get("path", ""),
        type=_FILE_KIND.get(e.get("type", ""), "other"),
        size=int(e.get("size", 0) or 0),
        mode=int(e.get("mode", 0) or 0),
        modified=_ts(_get(e, "modifiedAt", "modified_at")),
        symlink_target=_get(e, "symlinkTarget", "symlink_target") or None,
        owner=e.get("owner", ""),
        group=e.get("group", ""),
    )


XATTR_NAMESPACE = "user.e2b."
"""Where e2b's envd keeps file metadata (`user.e2b.<key>` attributes)."""


def _accepts(fn, name: str) -> bool:
    """Whether the SDK method `fn` takes the keyword `name` (an SDK that
    predates the argument lacks it, whatever the agent advertises)."""
    try:
        return name in inspect.signature(fn).parameters
    except (TypeError, ValueError):
        return False


def _stat_entry(e, *, meta: bool = False) -> FileStat:
    """A `cua.FileEntry` (the SDK's typed file calls) as a `FileStat`; `meta`:
    the call asked for the `user.e2b.` attributes."""
    ms = getattr(e, "modified_ms", None)
    return FileStat(
        name=e.name,
        path=e.path,
        type=_FILE_KIND.get(e.kind, "other"),
        size=int(e.size or 0),
        mode=int(e.mode or 0),
        modified=datetime.fromtimestamp(ms / 1000, timezone.utc) if ms is not None else None,
        symlink_target=e.symlink_target or None,
        owner=e.owner or "",
        group=e.group or "",
        metadata=dict(getattr(e, "metadata", None) or {}) if meta else None,
    )


SCROLLBACK_BYTES = 64 * 1024 * 1024
"""Output ring asked of cua-spacesd per command (the server clamps it to its
maximum): e2b never drops output, so the ring is sized for slow readers."""


def _scrollback(cua) -> dict:
    try:
        import inspect

        if "scrollback_bytes" in inspect.signature(cua.SpacesdCommand.__init__).parameters:
            raw = os.environ.get("CUA_E2B_SCROLLBACK_BYTES", "").strip()
            return {"scrollback_bytes": int(raw) if raw else SCROLLBACK_BYTES}
    except Exception:  # noqa: BLE001
        pass
    return {}


class OutputLost(Exception):
    """Command output was evicted from cua-spacesd's scrollback before the
    reader got it (mapped to a `SandboxException`)."""


class CuaProcess:
    def __init__(self, proc):
        self._p = proc
        self.pid = proc.pid()
        self._next_offset: Optional[int] = None

    async def next_event(self) -> Optional[ProcEvent]:
        ev = await self._p.next_event()
        if ev is None:
            return None
        kind = _KIND[ev.kind.name]
        if kind != "exit":
            # cua-spacesd keeps output in a bounded scrollback ring and never
            # blocks the process; a reader that falls behind by more than the
            # ring loses the evicted bytes, which shows as a gap in the output
            # offsets. e2b never drops output, so a gap is an error here, not
            # a silently shorter result.
            if self._next_offset is not None and ev.offset > self._next_offset:
                lost = ev.offset - self._next_offset
                raise OutputLost(
                    f"{lost} bytes of command output were dropped because the reader fell "
                    "behind cua-spacesd's scrollback ring; read output faster or write it to "
                    "a file in the sandbox"
                )
            self._next_offset = ev.offset + len(ev.data)
            return ProcEvent(kind=kind, data=bytes(ev.data))
        x = ev.exit
        return ProcEvent(
            kind="exit",
            exit_code=x.code if x else None,
            signal=x.signal if x else None,
            signal_name=getattr(x, "signal_name", None) if x else None,
            signal_number=getattr(x, "signal_number", None) if x else None,
            timed_out=bool(x and x.timed_out),
            error=x.error if x else None,
        )

    async def kill(self) -> bool:
        """SIGKILL to the process and its group (the SDK's `kill`)."""
        try:
            await self._p.kill()
            return True
        except Exception as e:  # noqa: BLE001
            if type(e).__name__ == "NotFound":
                return False
            raise

    async def write_stdin(self, data: bytes) -> None:
        await self._p.write_stdin(data)

    async def write_pty(self, data: bytes) -> None:
        await self._p.write_pty(data)

    async def resize(self, cols: int, rows: int) -> None:
        await self._p.resize(cols, rows)

    async def close_stdin(self) -> None:
        """Closes stdin. cua-spacesd closes an already closed stdin without
        error; e2b refuses it (the process no longer takes input), so an empty
        write first finds a stdin that is closed, not requested, or whose
        process exited (the same refusal a `write_stdin` gets)."""
        await self._p.write_stdin(b"")
        await self._p.close_stdin()

    async def detach(self) -> None:
        await self._p.detach()


class CuaDownload:
    def __init__(self, d, cleanup=None):
        self._d = d
        self._cleanup = cleanup
        self._ahead: List[Optional[bytes]] = []

    async def prefetch(self) -> None:
        """Reads the first chunk now, so a refusal surfaces at open."""
        c = await self._d.next_chunk()
        self._ahead.append(None if c is None else bytes(c))

    async def next_chunk(self) -> Optional[bytes]:
        if self._ahead:
            c = self._ahead.pop()
            if c is not None:
                return c
        else:
            c = await self._d.next_chunk()
        if c is None and self._cleanup:
            cleanup, self._cleanup = self._cleanup, None
            await cleanup()
        return None if c is None else bytes(c)

    def cancel(self) -> None:
        self._d.cancel()
        if self._cleanup:
            cleanup, self._cleanup = self._cleanup, None
            try:
                asyncio.get_running_loop().create_task(cleanup())
            except RuntimeError:
                pass


def _denied(e: BaseException) -> bool:
    return type(e).__name__ == "PermissionDenied"


def _missing(e: BaseException) -> bool:
    """A path that is not there (the file service's NotFound, or the same
    from a command run as root)."""
    return type(e).__name__ == "NotFound" or isinstance(e, FileNotFoundError)


class CuaMachine:
    """A Space (created), or a sandbox forked from a checkpoint.

    Commands run as e2b's `user` through the SDK's `SpacesdCommand.user`
    (which uses `sudo` where the driver cannot switch users). cua-spacesd's
    file service acts as its own user; e2b's file calls run as root and give
    what they create to `user`, so a call the file service refuses is
    retried as a command run as root (`mv` from a staged upload, `cat`,
    `rm`, `stat`), and what a call created is given to that user.

    A cua-spacesd that advertises `fs.run_as` (and an SDK that has the
    `user=` arguments) does both natively: a call acts as the user it is for,
    and the root retry is the same call with `user="root"` (no staging file,
    no helper scripts); `fs.xattr` stores and reports the metadata under
    `user.e2b.`. The features are read once per machine; an agent that
    advertises neither keeps the command-based paths above.
    """

    def __init__(
        self, client, name: str, env, *, space_id=None, sandbox=None, embedded=False, release=None
    ):
        self._c = client
        self.sandbox_id = name
        self._env = env
        self._space_id = space_id
        self._sandbox = sandbox
        self._embedded = embedded
        self._release = (
            release  # `direct:` machines: drops this sandbox's claim on the shared Space
        )
        self._forwards: dict = {}
        self._fork_lock = asyncio.Lock()
        self._driver_user: Optional[str] = None
        self._known_users: set = set()
        self._features: Optional[Dict[str, object]] = None

    # capabilities

    async def features(self) -> Dict[str, object]:
        """The agent's supported features by name (`GetCapabilities`, read
        once). Empty for an agent or SDK that reports none."""
        if self._features is None:
            try:
                caps = await self._env.capabilities()
            except AttributeError:
                self._features = {}  # an SDK without the call: nothing advertised
            except Exception:  # noqa: BLE001 - unreachable now: the legacy paths, ask again later
                return {}
            else:
                self._features = {f.name: f for f in caps.features if getattr(f, "supported", True)}
        return self._features

    async def run_as_native(self) -> bool:
        """File calls act as another user inside cua-spacesd (`fs.run_as`)."""
        return "fs.run_as" in await self.features() and _accepts(self._env.stat, "user")

    async def xattr_native(self) -> bool:
        """File metadata is stored and reported by cua-spacesd (`fs.xattr`)."""
        return "fs.xattr" in await self.features() and _accepts(self._env.stat, "xattr_namespace")

    def _kw(self, user: Optional[str]) -> dict:
        """`user=` for a typed file call acting as `user` (nothing: the
        agent's own user)."""
        return {"user": user} if user else {}

    # users

    async def driver_user(self) -> str:
        """The user cua-spacesd runs as (the sandbox's default user)."""
        if self._driver_user is None:
            import cua

            out = await self._env.run(
                cua.SpacesdCommand(program="id", args=["-un"], timeout_ms=30_000)
            )
            self._driver_user = bytes(out.stdout).decode().strip()
        return self._driver_user

    async def _other(self, user: Optional[str]) -> Optional[str]:
        """`user` when it is not the driver's own (so `sudo` is needed)."""
        if user is None:
            return None
        return None if user == await self.driver_user() else user

    async def _as(self, user: str, argv: List[str]) -> bytes:
        """Runs `argv` as `user` to completion; stdout, or the guest error."""
        import cua

        out = await self._env.run(
            cua.SpacesdCommand(program=argv[0], args=argv[1:], user=user, timeout_ms=120_000)
        )
        if out.exit.code != 0:
            msg = bytes(out.stderr).decode(errors="replace").strip()
            if "No such file" in msg or "cannot stat" in msg:
                raise FileNotFoundError(msg)
            raise PermissionError(msg or f"{argv[0]} exited {out.exit.code}")
        return bytes(out.stdout)

    # exec

    async def spawn(self, argv, *, env, cwd, user, timeout_ms, stdin, pty=None) -> CuaProcess:
        import cua

        cmd = cua.SpacesdCommand(
            program=argv[0],
            args=list(argv[1:]),
            env=dict(env),
            cwd=cwd,
            user=user,
            timeout_ms=timeout_ms,
            stdin=stdin,
            pty=cua.PtySize(cols=pty[0], rows=pty[1]) if pty else None,
            **_scrollback(cua),
        )
        return CuaProcess(await self._env.spawn(cmd))

    async def attach(self, pid: int) -> CuaProcess:
        import cua

        return CuaProcess(await self._env.attach(pid, None, cua.ReplayMode.NONE()))

    async def list_processes(self) -> List[dict]:
        return json.loads(await self._env.list_processes(False))

    # files
    #
    # e2b's file calls run as root inside the sandbox and give what they
    # create to `user`: a file the user could not touch is still read,
    # written and removed. cua-spacesd's file service acts as its own user,
    # so a refusal (PermissionDenied) is retried as a command run as root
    # (`cat`, `rm`, `mv` from a staged upload, `mkdir`, `mv`), and what a
    # call created is handed to `user` with `chown`.

    def _stage(self) -> str:
        return f"/tmp/.cua-e2b-{secrets.token_hex(8)}"

    async def has_user(self, name: str) -> bool:
        """Whether the guest has a user `name` (checked once per name)."""
        import cua

        if name in self._known_users:
            return True
        out = await self._env.run(
            cua.SpacesdCommand(program="id", args=["-u", "--", name], timeout_ms=30_000)
        )
        if out.exit.code != 0:
            return False
        self._known_users.add(name)
        return True

    async def _owner(self, user: Optional[str]) -> str:
        return user or await self.driver_user()

    async def _chown(self, user: str, paths: List[str]) -> None:
        if paths:
            await self._as("root", ["chown", f"{user}:", "--", *paths])

    async def _missing_dirs(self, d: str) -> List[str]:
        """The directories of the path `d` that do not exist yet, outermost
        first (what a write or make_dir is about to create)."""
        out: List[str] = []
        while d and d != "/":
            try:
                await self.stat(d, None, follow=True)
                break
            except Exception as e:  # noqa: BLE001
                if not _missing(e):
                    break
                out.append(d)
                d = posixpath.dirname(d)
        return out[::-1]

    async def _through_links(self, path: str) -> str:
        """The file a write to `path` lands on: an in-place open follows
        symbolic links, a rename into place would replace the link."""
        for _ in range(40):
            try:
                st = await self._stat(path, False)
            except Exception:  # noqa: BLE001
                return path
            if st.type != "symlink" or not st.symlink_target:
                return path
            path = posixpath.join(posixpath.dirname(path), st.symlink_target)
        return path

    async def upload(
        self,
        path: str,
        data: bytes,
        user: Optional[str] = None,
        *,
        local_path: Optional[str] = None,
        metadata: Optional[Dict[str, str]] = None,
    ) -> None:
        import cua

        native = await self.run_as_native()

        def options(as_user: Optional[str] = None):
            kw: dict = {"create_parents": True}
            if native and as_user:
                kw["user"] = as_user
            if metadata:
                kw["metadata"] = {k.lower(): v for k, v in metadata.items()}
                kw["xattr_namespace"] = XATTR_NAMESPACE
            return cua.UploadOptions(**kw)

        async def put(dest: str, as_user: Optional[str] = None) -> None:
            if local_path is not None:
                await self._env.upload_file(local_path, dest, options(as_user))
            else:
                await self._env.upload(dest, data, options(as_user))

        other = await self._other(user)
        path = await self._through_links(path)
        if native:
            try:
                await put(path, other)  # acts as the user: what it makes is the user's
                return
            except Exception as e:  # noqa: BLE001
                if not _denied(e):
                    raise
            made = await self._missing_dirs(posixpath.dirname(path))
            await put(path, "root")
            await self._chown(await self._owner(user), [*made, path])
            return
        made = await self._missing_dirs(posixpath.dirname(path)) if other else []
        try:
            await put(path)
        except Exception as e:  # noqa: BLE001
            if not _denied(e):
                raise
            made = await self._missing_dirs(posixpath.dirname(path))
            stage = self._stage()
            await put(stage)
            await self._as(
                "root",
                [
                    "sh",
                    "-c",
                    'mkdir -p -- "$(dirname -- "$1")" && mv -f -- "$2" "$1"',
                    "_",
                    path,
                    stage,
                ],
            )
            other = other or await self.driver_user()
        if other:
            await self._chown(other, [*made, path])

    async def download(self, path: str, user: Optional[str] = None) -> bytes:
        try:
            return bytes(await self._env.download(path))
        except Exception as e:  # noqa: BLE001
            if not _denied(e):
                raise
            if await self.run_as_native():
                return bytes(await self._env.download(path, user="root"))
            return await self._as("root", ["cat", "--", path])

    async def open_download(self, path: str, user: Optional[str] = None) -> CuaDownload:
        try:
            d = CuaDownload(await self._env.open_download(path, None))
            await d.prefetch()  # a refusal may only show on the first chunk
            return d
        except Exception as e:  # noqa: BLE001
            if not _denied(e):
                raise
        if await self.run_as_native():
            return CuaDownload(await self._env.open_download(path, None, user="root"))
        stage = self._stage()
        me = await self.driver_user()
        await self._as(
            "root", ["sh", "-c", 'cp -- "$1" "$2" && chown "$3": "$2"', "_", path, stage, me]
        )

        async def cleanup():
            try:
                await self._env.remove(stage, False)
            except Exception:  # noqa: BLE001
                pass

        return CuaDownload(await self._env.open_download(stage, None), cleanup)

    async def remove(self, path: str, user: Optional[str] = None) -> None:
        try:
            await self._env.remove(path, True)
        except Exception as e:  # noqa: BLE001
            if not _denied(e):
                raise
            if await self.run_as_native():
                await self._env.remove(path, True, user="root")
            else:
                await self._as("root", ["rm", "-rf", "--", path])

    async def _fs(self, method: str, request: dict) -> dict:
        return json.loads(await self._env.call_json(_FS + method, json.dumps(request)))

    # Runs as root when the file service is refused: `stat` / `lstat` of
    # argv[1], or (`ls`) its entries to depth argv[3].
    _STAT_PY = (
        "import json,os,pwd,grp,stat,sys,datetime\n"
        "def nm(f,i):\n"
        " try: return f(i)[0]\n"
        " except KeyError: return str(i)\n"
        "def e(p,fol=False):\n"
        " s=os.stat(p) if fol else os.lstat(p)\n"
        " t='FILE_TYPE_DIRECTORY' if stat.S_ISDIR(s.st_mode) else 'FILE_TYPE_SYMLINK' if stat.S_ISLNK(s.st_mode) else 'FILE_TYPE_FILE'\n"
        " r={'name':os.path.basename(p),'path':p,'type':t,'size':s.st_size,'mode':stat.S_IMODE(s.st_mode),"
        "'owner':nm(pwd.getpwuid,s.st_uid),'group':nm(grp.getgrgid,s.st_gid),"
        "'modifiedAt':datetime.datetime.fromtimestamp(s.st_mtime,datetime.timezone.utc).isoformat()}\n"
        " if t=='FILE_TYPE_SYMLINK': r['symlinkTarget']=os.readlink(p)\n"
        " return r\n"
        "def walk(p,d):\n"
        " out=[]\n"
        " for n in sorted(os.listdir(p)):\n"
        "  q=os.path.join(p,n)\n"
        "  x=e(q)\n"
        "  if x['type']=='FILE_TYPE_SYMLINK' and not os.path.exists(q): continue\n"
        "  out.append(x)\n"
        "  if d>1 and x['type']=='FILE_TYPE_DIRECTORY': out+=walk(q,d-1)\n"
        " return out\n"
        "m=sys.argv[2]\n"
        "print(json.dumps(walk(sys.argv[1],int(sys.argv[3])) if m=='ls' else e(sys.argv[1],m=='stat')))"
    )

    async def stat(
        self, path: str, user: Optional[str] = None, *, follow: bool = False
    ) -> FileStat:
        """`follow`: the target of a symbolic link, not the link itself."""
        st = await self._stat(path, follow)
        if st.type == "symlink":
            await self._link_targets([st], drop_broken=False)
        return st

    async def _typed_stat(self, path: str, follow: bool, user: Optional[str] = None) -> FileStat:
        kw: dict = {"no_follow": not follow}
        if user:
            kw["user"] = user
        meta = await self.xattr_native()
        if meta:
            kw["xattr_namespace"] = XATTR_NAMESPACE
        return _stat_entry(await self._env.stat(path, **kw), meta=meta)

    async def _stat(self, path: str, follow: bool) -> FileStat:
        if await self.run_as_native() or await self.xattr_native():
            try:
                return await self._typed_stat(path, follow)
            except Exception as e:  # noqa: BLE001
                if not _denied(e) or not await self.run_as_native():
                    raise
                return await self._typed_stat(path, follow, "root")
        try:
            r = await self._fs("Stat", {"path": path, "noFollowSymlinks": not follow})
            return _stat_json(r.get("entry") or {})
        except Exception as e:  # noqa: BLE001
            if not _denied(e):
                raise
            mode = "stat" if follow else "lstat"
            return _stat_json(
                json.loads(
                    await self._as("root", ["python3", "-c", self._STAT_PY, path, mode, "1"])
                )
            )

    async def _typed_list(
        self, path: str, depth: int, user: Optional[str] = None
    ) -> List[FileStat]:
        kw: dict = {}
        if user:
            kw["user"] = user
        meta = await self.xattr_native()
        if meta:
            kw["xattr_namespace"] = XATTR_NAMESPACE
        return [_stat_entry(e, meta=meta) for e in await self._env.list_dir(path, depth, **kw)]

    async def list_dir(self, path: str, depth: int, user: Optional[str] = None) -> List[FileStat]:
        if await self.run_as_native() or await self.xattr_native():
            try:
                rows = await self._typed_list(path, depth)
            except Exception as e:  # noqa: BLE001
                if not _denied(e) or not await self.run_as_native():
                    raise
                rows = await self._typed_list(path, depth, "root")
            return await self._link_targets(rows, drop_broken=True)
        try:
            out, token = [], ""
            while True:
                req = {"path": path, "depth": depth, "includeHidden": True}
                if token:
                    req["pageToken"] = token
                r = await self._fs("ListDir", req)
                out += [_stat_json(e) for e in r.get("entries", [])]
                token = _get(r, "nextPageToken", "next_page_token") or ""
                if not token:
                    break
        except Exception as e:  # noqa: BLE001
            if not _denied(e):
                raise
            rows = json.loads(
                await self._as("root", ["python3", "-c", self._STAT_PY, path, "ls", str(depth)])
            )
            return await self._link_targets([_stat_json(r) for r in rows], drop_broken=True)
        return await self._link_targets(out, drop_broken=True)

    async def _evaluated(self, e: FileStat) -> Optional[str]:
        """Where the symbolic link `e` finally points (Go's
        `filepath.EvalSymlinks`, which e2b reports as the target); `None`
        when that is gone."""
        p, target = e.path, e.symlink_target
        for _ in range(40):
            nxt = posixpath.normpath(posixpath.join(posixpath.dirname(p), target or ""))
            try:
                st = await self._stat(nxt, False)
            except Exception as err:  # noqa: BLE001
                if _missing(err):
                    return None
                return nxt
            if st.type != "symlink" or not st.symlink_target:
                return nxt
            p, target = nxt, st.symlink_target
        return None

    async def _link_targets(self, entries: List[FileStat], *, drop_broken: bool) -> List[FileStat]:
        """e2b reports a symbolic link's target resolved to the end of the
        chain (the link's own path when that is gone) and its listing leaves
        out a link whose target is gone."""
        links = [e for e in entries if e.type == "symlink"]
        if not links:
            return entries
        gone = set()
        for e, final in zip(links, await asyncio.gather(*(self._evaluated(e) for e in links))):
            if final is None:
                gone.add(e.path)
                e.symlink_target = e.path
            else:
                e.symlink_target = final
        return [e for e in entries if e.path not in gone] if drop_broken else entries

    async def make_dir(self, path: str, user: Optional[str] = None) -> None:
        other = await self._other(user)
        if await self.run_as_native():
            try:
                await self._env.make_dir(path, **self._kw(other))
                return
            except Exception as e:  # noqa: BLE001
                if not _denied(e):
                    raise
            made = await self._missing_dirs(path)
            await self._env.make_dir(path, user="root")
            await self._chown(await self._owner(user), made)
            return
        made = await self._missing_dirs(path) if other else []
        try:
            await self._env.make_dir(path)
        except Exception as e:  # noqa: BLE001
            if not _denied(e):
                raise
            made = await self._missing_dirs(path)
            await self._as("root", ["mkdir", "-p", "--", path])
            other = other or await self.driver_user()
        if other:
            await self._chown(other, made)

    async def rename(self, old: str, new: str, user: Optional[str] = None) -> FileStat:
        other = await self._other(user)
        if await self.run_as_native():
            move = dict(overwrite=True, create_parents=True)
            try:
                return _stat_entry(await self._env.move_path(old, new, **move, **self._kw(other)))
            except Exception as e:  # noqa: BLE001
                if not _denied(e):
                    raise
            made = await self._missing_dirs(posixpath.dirname(new))
            st = _stat_entry(await self._env.move_path(old, new, **move, user="root"))
            await self._chown(await self._owner(user), made)
            return st
        made = await self._missing_dirs(posixpath.dirname(new)) if other else []
        try:
            r = await self._fs(
                "Move",
                {"source": old, "destination": new, "overwrite": True, "createParents": True},
            )
            st = _stat_json(r.get("entry") or {})
        except Exception as e:  # noqa: BLE001
            if not _denied(e):
                raise
            made = await self._missing_dirs(posixpath.dirname(new))
            await self._as(
                "root",
                [
                    "sh",
                    "-c",
                    'mkdir -p -- "$(dirname -- "$2")" && mv -f -- "$1" "$2"',
                    "_",
                    old,
                    new,
                ],
            )
            other = other or await self.driver_user()
            st = await self.stat(new, None)
        if other:
            await self._chown(other, made)
        return st

    async def watch(self, path: str, recursive: bool, user: Optional[str] = None) -> str:
        """Opens a watcher; with `fs.run_as` it is opened as `user` (the
        watcher sees what that user can see), otherwise as the agent's user."""
        other = await self._other(user)
        if other and await self.run_as_native() and _accepts(self._env.call_json, "user"):
            r = json.loads(
                await self._env.call_json(
                    _FS + "CreateWatcher",
                    json.dumps({"path": path, "recursive": recursive}),
                    user=other,
                )
            )
        else:
            r = await self._fs("CreateWatcher", {"path": path, "recursive": recursive})
        return _get(r, "watcherId", "watcher_id")

    async def watch_events(self, watcher_id: str) -> List[FsEvent]:
        r = await self._fs("GetWatcherEvents", {"watcherId": watcher_id, "maxEvents": 1000})
        return [
            FsEvent(
                path=e.get("path", ""),
                type=_FS_EVENT.get(e.get("type", ""), "write"),
                old_path=_get(e, "oldPath", "old_path", default="") or "",
            )
            for e in r.get("events", [])
        ]

    async def unwatch(self, watcher_id: str) -> None:
        await self._fs("RemoveWatcher", {"watcherId": watcher_id})

    # egress rules and signed URLs

    async def set_egress(self, allow: List[str], deny: List[str]) -> None:
        """Replaces the sandbox's egress rules (IP addresses and CIDR blocks;
        the core applies them inside the Space). Empty lists remove them."""
        import cua

        if not hasattr(cua, "EgressPolicy"):
            from e2b.exceptions import InvalidArgumentException

            raise InvalidArgumentException(
                "cua-e2b: egress rules need a cua SDK with Spaces.set_egress; this one has "
                "none. Upgrade `cua`, or use allow_internet_access=False for no network"
            )
        # a sandbox forked from a checkpoint (older SDKs) is the Space `local:<name>`
        ref = self._space_id or f"local:{self.sandbox_id}"
        await self._c.spaces().set_egress(ref, cua.EgressPolicy(allow=list(allow), deny=list(deny)))

    def endpoint(self) -> Optional[str]:
        """The cua-spacesd URL this machine reaches the sandbox on."""
        try:
            return self._env.endpoint()
        except Exception:  # noqa: BLE001
            return None

    async def signed_url(self, path: str, method: str, ttl: int, user: Optional[str]) -> str:
        """A signed `/files` URL (cua-spacesd's `CreateSignedUrl`) for `method`
        (`GET` or `PUT`) on `path`, valid `ttl` seconds. A user other than
        the one cua-spacesd runs as is part of the signature (`run_as`: the
        file service acts as that user); an upload creates missing parent
        directories, as e2b's envd does."""
        word = {"GET": "SIGNED_URL_METHOD_GET", "PUT": "SIGNED_URL_METHOD_PUT"}[method]
        request = {"path": path, "method": word, "ttl": f"{ttl}s"}
        other = await self._other(user)
        native = (
            bool(other) and await self.run_as_native() and _accepts(self._env.call_json, "user")
        )
        if other and not native:
            request["runAs"] = other
        if method == "PUT":
            request["createParents"] = True
        reply = json.loads(
            await self._env.call_json(
                _FS + "CreateSignedUrl", json.dumps(request), **({"user": other} if native else {})
            )
        )
        if other and not native and "&ru=" not in _get(reply, "urlPath", "url_path"):
            # a cua-spacesd that predates `run_as` ignores it: the URL would act
            # as the default user, never silently as someone else
            from e2b.exceptions import InvalidArgumentException

            raise InvalidArgumentException(
                f"cua-e2b: this sandbox's cua-spacesd cannot sign file URLs for user={user!r} "
                f"(it signs as {await self.driver_user()!r}); rebuild the image with a current "
                f"cua-spacesd, or use files.read/files.write(user={user!r})"
            )
        base = (self._env.endpoint() or "").rstrip("/")
        if "/v1/spaces/" in base:
            # The daemon's proxy route wants the daemon's credentials; a signed
            # URL must reach cua-spacesd itself, through the tunnel.
            base = "http://" + await self.forward(SPACESD_PORT)
        return base + _get(reply, "urlPath", "url_path")

    # metrics

    async def metrics(self) -> dict:
        """One resource sample from cua-spacesd's `SystemService.Metrics`
        (CPU %, memory, root disk) plus the page cache from the guest's
        /proc/meminfo, which the service does not report."""
        import cua

        m = json.loads(await self._env.call_json("/cua.env.v1.SystemService/Metrics", "{}"))
        out = await self._env.run(
            cua.SpacesdCommand(
                program="sh",
                args=["-c", "awk '/^Cached:/{print $2*1024}' /proc/meminfo"],
                timeout_ms=15_000,
            )
        )
        try:
            cache = int(bytes(out.stdout).decode().strip() or 0)
        except ValueError:
            cache = 0
        return {
            "cpu_count": int(_get(m, "cpuCount", "cpu_count", default=0)),
            "cpu_pct": float(_get(m, "cpuPercent", "cpu_percent", default=0.0)),
            "mem_total": int(_get(m, "memoryTotalBytes", "memory_total_bytes", default=0)),
            "mem_used": int(_get(m, "memoryUsedBytes", "memory_used_bytes", default=0)),
            "mem_cache": cache,
            "disk_total": int(_get(m, "diskTotalBytes", "disk_total_bytes", default=0)),
            "disk_used": int(_get(m, "diskUsedBytes", "disk_used_bytes", default=0)),
        }

    # ports

    async def forward(self, port: int) -> str:
        if port not in self._forwards:
            bind = os.environ.get("CUA_E2B_BIND") or None
            # cua-spacesd's tunnel: the same path for a Space on this
            # machine, one of your machines or the cloud. The forward closes
            # when its handle is dropped, so it is kept.
            fwd = await self._env.forward_tcp(port, bind)
            self._forwards[port] = fwd
        return self._forwards[port].local_addr()

    # lifecycle

    async def _sandbox_handle(self):
        if self._sandbox is None:
            self._sandbox = await self._c.sandboxes().connect(self.sandbox_id)
        return self._sandbox

    async def set_lifetime(self, seconds: float, on_expire: str) -> bool:
        """Sets the lifetime the cua daemon enforces. `False`: an embedded
        runtime (no daemon), so the caller keeps its own timer too."""
        if self._release is not None:
            # A `direct:` sandbox shares its Space with every other sandbox on
            # the machine, so a Space lifetime would end them all: the caller
            # keeps this sandbox's time itself (False).
            return False
        secs = max(0, int(seconds + 0.999))
        if self._space_id is not None:
            await self._c.spaces().set_lifetime(self._space_id, secs, on_expire)
        else:
            await (await self._sandbox_handle()).set_lifetime(secs, on_expire)
        return not self._embedded

    async def checkpoint(self, name: str, cold: bool = False) -> str:
        """Saves this VM as checkpoint `name` (pinned, so it lives until it
        is deleted, as an e2b snapshot does). `cold`: sandboxes made from it
        cold-boot from its disk."""
        if self._space_id is not None and _HAS_SPACE_FORK:
            space = await self._c.spaces().space(self._space_id)
            saved = (await space.checkpoint(name, True)).name
        else:
            saved = (await self._c.sandboxes().checkpoint(self.sandbox_id, name)).name
        _save_record(
            f"snapshot/{saved}",
            {
                "cold": cold,
                "source": self.sandbox_id,
                "created": datetime.now(timezone.utc).isoformat(),
            },
        )
        return saved

    async def fork(self, name: str) -> "CuaMachine":
        """A running copy of this VM, memory included (copy-on-write where
        the runtime can). The original keeps running."""
        if self._space_id is not None and _HAS_SPACE_FORK:
            space = await (await self._c.spaces().space(self._space_id)).fork(name)
            info = space.info()
            return CuaMachine(
                self._c, info.name, space.spacesd(), space_id=info.id, embedded=self._embedded
            )
        ck = f"{name}-src"
        async with self._fork_lock:  # one checkpoint of this VM at a time
            await self._c.sandboxes().checkpoint(self.sandbox_id, ck)
            try:
                sb = await self._c.sandboxes().fork(ck, name)
            finally:
                await self._c.sandboxes().delete_checkpoint(ck)
        return CuaMachine(
            self._c, sb.name(), await sb.spacesd(None), sandbox=sb, embedded=self._embedded
        )

    async def _is_paused(self) -> bool:
        return await CuaBackend(self._c).state(self.sandbox_id) == "paused"

    async def suspend(self, cold: bool = False) -> None:
        """Pauses the VM (memory kept). `cold`: the next resume cold-boots
        it from its disk instead (processes and memory do not survive)."""
        self._annotate(resume="cold" if cold else None)
        if self._space_id is not None:
            await self._c.spaces().stop(self._space_id)
        else:
            await (await self._sandbox_handle()).suspend()

    async def resume(self, reboot: bool = False) -> None:
        was_paused = await self._is_paused()
        cold = reboot or _load_records().get(self.sandbox_id, {}).get("resume") == "cold"
        if self._space_id is not None:
            await self._c.spaces().start(self._space_id)
        else:
            await (await self._sandbox_handle()).resume()
        if was_paused:
            self._annotate(resume=None)
            if cold:
                await self.reboot()

    def _annotate(self, **fields) -> None:
        try:
            _update_record(self.sandbox_id, **fields)
        except OSError:
            pass

    async def reboot(self) -> None:
        """Restarts the guest OS: its processes and memory are gone, its
        disk stays (a cold boot). Returns once the guest answers again."""
        import cua

        async def probe() -> str:
            out = await self._env.run(
                cua.SpacesdCommand(
                    program="sh",
                    args=["-c", "cat /proc/sys/kernel/random/boot_id 2>/dev/null || uptime"],
                    timeout_ms=15_000,
                )
            )
            return bytes(out.stdout).decode(errors="replace").strip()

        before = await probe()
        # Detached from this call, a second later, so the reply gets out.
        await self._env.run(
            cua.SpacesdCommand(
                program="sh",
                args=[
                    "-c",
                    "sync; nohup sh -c 'sleep 1; reboot || systemctl reboot' >/dev/null 2>&1 &",
                ],
                user="root",
                timeout_ms=30_000,
            )
        )
        deadline = asyncio.get_running_loop().time() + 300
        await asyncio.sleep(3)
        while True:
            try:
                if self._space_id is not None:
                    # the connection died with the guest: take a fresh one
                    space = await self._c.spaces().space(self._space_id)
                    self._env = space.spacesd()
                    self._forwards.clear()
                after = await probe()
                if after and after != before:
                    return
            except Exception:  # noqa: BLE001 - not up yet
                pass
            if asyncio.get_running_loop().time() > deadline:
                raise TimeoutError("the sandbox did not come back from a cold boot")
            await asyncio.sleep(2)

    async def delete(self) -> bool:
        for fwd in list(self._forwards.values()):
            try:
                await fwd.stop()
            except Exception:  # noqa: BLE001
                pass
        self._forwards.clear()
        try:
            if self._release is not None:
                # a `direct:` machine is one Space shared by every sandbox: only
                # the last one to go removes it
                return await self._release(self.sandbox_id)
            if self._space_id is not None:
                # the core deletes by id idempotently: ask whether it was there
                if await CuaBackend(self._c).state(self.sandbox_id) is None:
                    return False
                await self._c.spaces().delete(self._space_id)
            else:
                await (await self._sandbox_handle()).delete()
            return True
        except Exception as e:  # noqa: BLE001
            if type(e).__name__ == "NotFound":
                return False
            raise


SPACESD_PORT = 3211  # where cua-spacesd listens in the guest
_OFF_STATES = ("suspended", "stopped", "off")


# Spaces.fork / SpaceCreateOptions.forkable (copy-on-write forks of pinned
# checkpoints); older SDKs fork through sandboxes().fork.
def _stopped(sandbox) -> bool:
    """A sandbox of the local runtime that is not running (suspended)."""
    status = str(sandbox.status).lower()
    return "suspend" in status or "stopped" in status


def _has_space_fork() -> bool:
    try:
        import inspect

        import cua

        return hasattr(cua.Spaces, "fork") and (
            "forkable" in inspect.signature(cua.SpaceCreateOptions.__init__).parameters
        )
    except Exception:  # noqa: BLE001
        return False


_HAS_SPACE_FORK = _has_space_fork()

# Templates that name an OS: Cua's canonical image for it.
OS_TEMPLATES = ("linux", "macos", "windows")

# Sandboxes this package creates are named `e2b-<hex>`; `Sandbox.list` lists
# them. Their template and creation time are kept next to the Spaces
# registry, so every process on this machine sees them.
NAME_PREFIX = "e2b-"


def _records_path() -> str:
    home = os.environ.get("CUA_HOME") or os.path.expanduser("~/.cua")
    return os.path.join(home, "e2b-sandboxes.json")


def _load_records() -> dict:
    try:
        with open(_records_path()) as f:
            return json.load(f)
    except (OSError, ValueError):
        return {}


def _save_record(sandbox_id: str, record: Optional[dict]) -> None:
    """Sets or drops one record. The read-modify-write is serialised across
    processes with an advisory lock, so many rollout workers on one machine
    never lose each other's updates."""
    path = _records_path()
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with _records_lock(path):
        records = _load_records()
        if record is None:
            records.pop(sandbox_id, None)
        else:
            records[sandbox_id] = record
        tmp = f"{path}.tmp-{secrets.token_hex(4)}"
        with open(tmp, "w") as f:
            json.dump(records, f)
        os.replace(tmp, path)


def _update_record(sandbox_id: str, **fields) -> None:
    """Merges `fields` into one record (a `None` value drops the field) under
    the records lock."""
    path = _records_path()
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with _records_lock(path):
        records = _load_records()
        record = records.get(sandbox_id, {})
        for k, v in fields.items():
            if v is None:
                record.pop(k, None)
            else:
                record[k] = v
        records[sandbox_id] = record
        tmp = f"{path}.tmp-{secrets.token_hex(4)}"
        with open(tmp, "w") as f:
            json.dump(records, f)
        os.replace(tmp, path)


@contextlib.contextmanager
def _records_lock(path: str):
    try:
        import fcntl
    except ImportError:  # Windows: single-process use
        yield
        return
    with open(path + ".lock", "a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        try:
            yield
        finally:
            fcntl.flock(lock, fcntl.LOCK_UN)


def _has_core_labels() -> bool:
    try:
        import inspect

        import cua

        params = inspect.signature(cua.SpaceCreateOptions.__init__).parameters
        return "labels" in params and "egress" in params
    except Exception:  # noqa: BLE001
        return False


_HAS_EGRESS = _has_core_labels()


def _core_create_options(egress: Optional[dict]) -> dict:
    """Space labels (they mark the Space as an e2b sandbox for every Cua tool)
    and egress rules, which the core applies before create returns."""
    if not _HAS_EGRESS:
        return {}
    import cua

    out: dict = {"labels": {"cua.e2b": "sandbox"}}
    if egress:
        out["egress"] = cua.EgressPolicy(allow=list(egress["allow"]), deny=list(egress["deny"]))
    return out


def _forkable() -> bool:
    """New Spaces can be checkpointed cheaply (`create_snapshot` is a
    milliseconds copy-on-write fork source). `CUA_E2B_FORKABLE=0` turns it
    off for fleets that never snapshot."""
    return os.environ.get("CUA_E2B_FORKABLE", "1").strip().lower() not in ("0", "false", "no")


class CuaBackend:
    """Sandboxes as Cua Spaces, through the cua SDK."""

    def __init__(self, client=None):
        self._c = client
        self._embedded = False
        self._guard = threading.Lock()
        # `CUA_E2B_ON=direct:<url>`: every sandbox is the same machine (one Space);
        # name -> Space id of the sandboxes this process holds on it
        self._direct: Dict[str, tuple] = {}
        self._direct_claims: Dict[str, int] = {}  # machine URL -> sandboxes (and creates) on it
        self._opening: Optional[concurrent.futures.Future] = None

    async def client(self):
        """The cua client, opened once for the process. The sync API runs on
        its own event-loop thread, so the first caller (from any loop) opens
        it and the others wait on that result."""
        if self._c is not None:
            return self._c
        with self._guard:
            fut = self._opening
            owner = fut is None
            if owner:
                fut = self._opening = concurrent.futures.Future()
        if not owner:
            return await asyncio.wrap_future(fut)
        try:
            import cua

            mode = os.environ.get("CUA_E2B_MODE", "auto").strip() or "auto"
            address = os.environ.get("CUA_E2B_DAEMON") or None
            if mode == "embedded":
                self._c, self._embedded = cua.embedded(), True
            elif mode == "daemon":
                self._c = cua.connect(address)
            else:
                self._c = await self._daemon_or_embedded(cua, address)
            fut.set_result(self._c)
            return self._c
        except BaseException as e:
            with self._guard:
                self._opening = None  # a later call may try again
            fut.set_exception(e)
            raise

    async def _daemon_or_embedded(self, cua, address):
        """The running cua daemon; else one started now with the bundled
        CLI (so sandbox lifetimes outlive this process); else in process."""
        for attempt in range(2):
            try:
                c = cua.connect(address)
                await c.info()
                return c
            except Exception:  # noqa: BLE001
                if attempt or address:
                    break
                cli = os.environ.get("CUA_BIN")
                if not cli:
                    try:
                        from cua._cli import binary

                        cli = str(binary())
                    except Exception:  # noqa: BLE001
                        cli = None
                if not cli or not os.path.exists(cli):
                    break
                await asyncio.to_thread(
                    subprocess.run,
                    [cli, "daemon", "start"],
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                    timeout=60,
                )
        self._embedded = True
        return cua.embedded()

    async def _checkpoint_names(self, c) -> set:
        try:
            return {s.name for s in await c.sandboxes().checkpoints()}
        except Exception:  # noqa: BLE001 - a location without checkpoints
            return set()

    async def create(
        self,
        template: Optional[str],
        *,
        network: Optional[str] = None,
        egress: Optional[dict] = None,
    ) -> CuaMachine:
        import cua

        c = await self.client()
        name = f"{NAME_PREFIX}{secrets.token_hex(5)}"
        on = os.environ.get("CUA_E2B_ON", "auto").strip() or "auto"
        if on.startswith("direct:"):
            # An existing machine: added as a Space, never created. Every
            # sandbox is the same Space, so the claims on it are counted here
            # (before the add, so a kill racing a create cannot remove it).
            url = on[len("direct:") :]
            with self._guard:
                self._direct_claims[url] = self._direct_claims.get(url, 0) + 1
            try:
                info = await c.spaces().add(url, os.environ.get("CUA_E2B_TOKEN") or None, name)
                space = await c.spaces().space(info.id)
            except BaseException:
                with self._guard:
                    self._direct_claims[url] -= 1
                raise
            with self._guard:
                self._direct[name] = (info.id, url)
            _save_record(
                name,
                {
                    "template": template,
                    "space_id": info.id,
                    "started_at": datetime.now(timezone.utc).isoformat(),
                },
            )
            return CuaMachine(
                c,
                name,
                space.spacesd(),
                space_id=info.id,
                embedded=self._embedded,
                release=self._release_direct,
            )
        trec = self.template(template) if template else None
        if trec and trec.get("kind") == "snapshot":
            template = trec["snapshot"]
        source = resolve_template(template)
        if source in OS_TEMPLATES:
            # `template="macos"` / `"windows"` / `"linux"`: Cua's own image
            # of that OS (a macOS VM needs a Mac host, placed by `on`).
            source = cua.canonical_image(source, None)
        if source and source in await self._checkpoint_names(c):
            # A fork keeps its checkpoint's network: a world baked offline
            # stays offline. With copy-on-write forks (Spaces.fork) the new
            # sandbox is a Space like any other; older SDKs fork a sandbox.
            if _HAS_SPACE_FORK:
                space = await c.spaces().fork(source, name)
                info = space.info()
                m = CuaMachine(
                    c, info.name, space.spacesd(), space_id=info.id, embedded=self._embedded
                )
            else:
                sb = await c.sandboxes().fork(source, name)
                env = await sb.spacesd(None)
                m = CuaMachine(c, sb.name(), env, sandbox=sb, embedded=self._embedded)
            if _load_records().get(f"snapshot/{source}", {}).get("cold"):
                await m.reboot()  # a filesystem-only snapshot: a cold boot
        else:
            created = await c.spaces().create(
                cua.SpaceCreateOptions(
                    image=source,
                    on=on,
                    kind=os.environ.get("CUA_E2B_KIND", "vm").strip() or "vm",
                    name=name,
                    cpus=_size("CUA_E2B_CPUS", "cpus"),
                    memory_mb=_size("CUA_E2B_MEMORY_MB", "memory_mb"),
                    disk_gb=_env_int("CUA_E2B_DISK_GB"),
                    wait=True,
                    network=network,
                    **({"forkable": _forkable()} if _HAS_SPACE_FORK else {}),
                    **_core_create_options(egress),
                )
            )
            info = created.space
            if _HAS_EGRESS:
                egress = None  # applied by the core, before create returned
            space = await c.spaces().space(info.id)
            m = CuaMachine(c, info.name, space.spacesd(), space_id=info.id, embedded=self._embedded)
        if egress:
            try:
                await m.set_egress(egress["allow"], egress["deny"])
            except BaseException:
                await m.delete()
                raise
        _save_record(
            m.sandbox_id,
            {
                "template": template,
                "space_id": m._space_id,
                "started_at": datetime.now(timezone.utc).isoformat(),
            },
        )
        return m

    # templates (e2b's `Template.build`): a recipe kept under a name

    def template(self, name: str) -> Optional[dict]:
        return _load_records().get(f"template/{name}")

    async def save_template(self, name: str, record: dict) -> None:
        """Keeps `record` as template `name`."""
        _save_record(f"template/{name}", record)

    async def delete_template(self, name: str) -> bool:
        rec = self.template(name)
        if rec is None:
            return False
        if rec.get("kind") == "snapshot":
            await self.delete_snapshot(rec["snapshot"])
        _save_record(f"template/{name}", None)
        return True

    async def connect(self, sandbox_id: str, reboot: bool = False) -> CuaMachine:
        """Reconnects to a sandbox, resuming it if it is paused (e2b's
        `connect`). A paused sandbox cold-boots when `reboot` is set or it
        was paused filesystem-only."""
        c = await self.client()
        if sandbox_id in self._direct:
            space = await c.spaces().space(self._direct[sandbox_id][0])
            return CuaMachine(
                c,
                sandbox_id,
                space.spacesd(),
                space_id=self._direct[sandbox_id][0],
                embedded=self._embedded,
                release=self._release_direct,
            )
        record = _load_records().get(sandbox_id, {})
        ref = record.get("space_id") or sandbox_id
        cold = reboot or record.get("resume") == "cold"
        try:
            space, resumed = await self._space_resumed(c, ref)
            info = space.info()
            m = CuaMachine(c, info.name, space.spacesd(), space_id=info.id, embedded=self._embedded)
        except Exception as e:  # noqa: BLE001 - a forked sandbox is not a Space
            if type(e).__name__ != "NotFound":
                raise
            sb = await c.sandboxes().connect(sandbox_id)
            return CuaMachine(
                c, sb.name(), await sb.spacesd(None), sandbox=sb, embedded=self._embedded
            )
        if resumed:
            m._annotate(resume=None)
            if cold:
                await m.reboot()
        return m

    @staticmethod
    async def _space_resumed(c, ref: str):
        """The Space, and whether it had to be resumed."""
        try:
            return await c.spaces().space(ref), False
        except Exception as e:  # noqa: BLE001
            text = str(e).lower()
            if type(e).__name__ != "InvalidArgument" or not (
                "paused" in text or "suspended" in text
            ):
                raise
        await c.spaces().start(ref)
        return await c.spaces().space(ref), True

    async def list_snapshots(self) -> List[dict]:
        """The snapshots `create_snapshot` made: `{name, source, created}`."""
        c = await self.client()
        have = await self._checkpoint_names(c)
        return [
            {"name": k[len("snapshot/") :], **v}
            for k, v in _load_records().items()
            if k.startswith("snapshot/") and k[len("snapshot/") :] in have
        ]

    async def delete_snapshot(self, name: str) -> bool:
        """Deletes a snapshot (its forks keep running). `False`: none."""
        c = await self.client()
        if name not in await self._checkpoint_names(c):
            return False
        if hasattr(c.sandboxes(), "pin_checkpoint"):
            await c.sandboxes().pin_checkpoint(name, False)
        await c.sandboxes().delete_checkpoint(name)
        _save_record(f"snapshot/{name}", None)
        return True

    async def _release_direct(self, name: str) -> bool:
        """Forgets a `direct:` sandbox; the shared Space is removed from the
        registry when the last claim on it goes. `False`: it was not held."""
        with self._guard:
            held = self._direct.pop(name, None)
            if held is None:
                return False
            space_id, url = held
            self._direct_claims[url] -= 1
            last = self._direct_claims[url] <= 0
        _save_record(name, None)
        if last:
            c = await self.client()
            try:
                await c.spaces().delete(space_id)
            except Exception as e:  # noqa: BLE001
                if type(e).__name__ != "NotFound":
                    raise
        return True

    async def state(self, sandbox_id: str) -> Optional[str]:
        """`"running"` or `"paused"`; `None` if there is no such sandbox."""
        c = await self.client()
        if sandbox_id in self._direct:
            return "running"
        for s in await self._spaces(c, _load_records()):
            if s.name == sandbox_id:
                return "paused" if await self._off(c, s) else "running"
        for sb in await c.sandboxes().list("local"):
            if sb.name == sandbox_id:
                return "paused" if _stopped(sb) else "running"
        return None

    @staticmethod
    async def _off(c, space) -> bool:
        """Whether a Space is paused. A Space resolved by id (the signed-out
        fallback of `_spaces`) carries no power state: its sandbox does."""
        if space.power_state:
            return space.power_state in _OFF_STATES
        for sb in await c.sandboxes().list("local"):
            if sb.name == space.name:
                return _stopped(sb)
        return False

    async def kill(self, sandbox_id: str) -> bool:
        """Deletes a sandbox by id without connecting to it (it may be
        paused). `False` if it was gone."""
        c = await self.client()
        if sandbox_id in self._direct:
            return await self._release_direct(sandbox_id)
        recorded = _load_records().get(sandbox_id, {}).get("space_id") or sandbox_id
        if await self.state(sandbox_id) is None:
            return False
        for ref in dict.fromkeys((recorded, f"local:{sandbox_id}", sandbox_id)):
            try:
                await c.spaces().delete(ref)
                return True
            except Exception as e:  # noqa: BLE001
                if type(e).__name__ != "NotFound":
                    raise
        try:
            await (await c.sandboxes().connect(sandbox_id)).delete()
            return True
        except Exception as e:  # noqa: BLE001
            if type(e).__name__ == "NotFound":
                return False
            raise

    def record(self, sandbox_id: str) -> dict:
        """What this machine knows about a sandbox (template, metadata, start
        and end time, ...)."""
        return dict(_load_records().get(sandbox_id, {}))

    def annotate(self, sandbox_id: str, **fields) -> None:
        """Adds `fields` (metadata, ...) to a sandbox's record."""
        try:
            _update_record(sandbox_id, **fields)
        except OSError:
            pass

    def forget(self, sandbox_id: str) -> None:
        try:
            _save_record(sandbox_id, None)
        except OSError:
            pass

    @staticmethod
    async def _spaces(c, records: dict) -> list:
        """The Spaces of this machine. When the account's other machines cannot
        be listed (signed out / relay refused), the Spaces this machine created
        are resolved one by one from the records, so listing never depends on
        a network the sandboxes do not use."""
        try:
            return list(await c.spaces().list())
        except Exception as e:  # noqa: BLE001
            if type(e).__name__ != "Unauthenticated":
                raise
        out = []
        for name, rec in records.items():
            ref = rec.get("space_id") if isinstance(rec, dict) else None
            if not ref or not name.startswith(NAME_PREFIX):
                continue
            try:
                out.append(await c.spaces().resolve(ref))
            except Exception:  # noqa: BLE001 - gone
                continue
        return out

    async def list(self) -> List[MachineInfo]:
        c = await self.client()
        records = _load_records()
        out = []
        seen = set()
        shared = {space_id for space_id, _ in self._direct.values()}
        for s in await self._spaces(c, records):
            if not s.name.startswith(NAME_PREFIX) or s.id in shared:
                continue  # a `direct:` machine is listed once per sandbox below
            seen.add(s.name)
            r = dict(records.get(s.name, {}))
            # what the core knows beats the records file
            created = getattr(s, "created_at_unix", 0)
            if created:
                r["started_at"] = datetime.fromtimestamp(created, timezone.utc).isoformat()
            expires = getattr(s, "expires_at_unix", 0)
            if expires:
                r["end_at"] = datetime.fromtimestamp(expires, timezone.utc).isoformat()
            out.append(
                MachineInfo(
                    sandbox_id=s.name,
                    template=r.get("template"),
                    started_at=_ts(r.get("started_at")),
                    state="paused" if await self._off(c, s) else "running",
                    extra=r,
                )
            )
        for name in self._direct:  # `direct:` sandboxes are one shared Space
            if name not in seen:
                seen.add(name)
                r = dict(records.get(name, {}))
                out.append(
                    MachineInfo(
                        sandbox_id=name,
                        template=r.get("template"),
                        started_at=_ts(r.get("started_at")),
                        state="running",
                        extra=r,
                    )
                )
        for sb in await c.sandboxes().list("local"):
            if sb.name.startswith(NAME_PREFIX) and sb.name not in seen:
                r = records.get(sb.name, {})
                out.append(
                    MachineInfo(
                        sandbox_id=sb.name,
                        template=r.get("template"),
                        started_at=_ts(r.get("started_at")),
                        state="paused" if _stopped(sb) else "running",
                        extra=r,
                    )
                )
        return out

"""An in-memory backend: scripted guest commands and files, no VMs, no host
processes."""

from __future__ import annotations

import asyncio
import itertools
import json
import types
from typing import Any, Dict, List, Optional

from e2b._backend import FileStat, FsEvent, MachineInfo, ProcEvent


class CuaError(Exception):
    pass


class NotFound(CuaError):
    pass


class Timeout(CuaError):
    pass


class Transport(CuaError):
    pass


_pids = itertools.count(100)


class FakeProcess:
    """Behaviour comes from the command line:

    - ``echo <text>``: prints text, exits 0;
    - ``fail <code>``: prints "out" / "err", exits with `code`;
    - ``chunks``: stdout b"he", b"llo \\xc3", b"\\xa9\\n" (a split UTF-8
      char), stderr b"warn", exits 0;
    - ``reply``: prints a JSON line, then keeps running until killed;
    - ``forever``: runs until killed or its timeout;
    - ``hang-start``: never acknowledges the start (see FakeMachine).
    """

    def __init__(self, cmd: str, timeout_ms: Optional[int]):
        self.pid = next(_pids)
        self.cmd = cmd
        self.queue: asyncio.Queue = asyncio.Queue()
        self.killed = False
        self.detached = False
        self.stdin: List[bytes] = []
        self.pty_in: List[bytes] = []
        self.size = None
        self.done = False
        word, _, arg = cmd.partition(" ")
        if word == "echo":
            self._put("stdout", (arg + "\n").encode())
            self._exit(0)
        elif word == "fail":
            self._put("stdout", b"out")
            self._put("stderr", b"err")
            self._exit(int(arg))
        elif word == "chunks":
            for c in (b"he", b"llo \xc3", b"\xa9\n"):
                self._put("stdout", c)
            self._put("stderr", b"warn")
            self._exit(0)
        elif word == "reply":
            self._put("stdout", b'{"ok": true}\n')
        elif word == "forever":
            if timeout_ms is not None:
                asyncio.get_running_loop().call_later(
                    timeout_ms / 1000, lambda: self._exit(None, signal="kill", timed_out=True)
                )
        else:
            self._put("stderr", f"bash: {word}: command not found\n".encode())
            self._exit(127)

    def _put(self, kind, data):
        self.queue.put_nowait(ProcEvent(kind=kind, data=data))

    def _exit(self, code, signal=None, timed_out=False):
        if self.done:
            return
        self.done = True
        self.queue.put_nowait(
            ProcEvent(kind="exit", exit_code=code, signal=signal, timed_out=timed_out)
        )

    async def next_event(self):
        if self.detached:
            return None
        ev = await self.queue.get()
        return ev

    async def kill(self) -> bool:
        if self.done:
            return False
        self.killed = True
        self._exit(None, signal="kill")
        return True

    async def write_stdin(self, data: bytes) -> None:
        self.stdin.append(data)

    async def write_pty(self, data: bytes) -> None:
        self.pty_in.append(data)
        # An echoing terminal.
        self._put("pty", data)

    async def resize(self, cols: int, rows: int) -> None:
        self.size = (cols, rows)

    async def close_stdin(self) -> None:
        pass

    async def detach(self) -> None:
        self.detached = True


class Attached:
    """A second handle on a process (detaching it leaves the first)."""

    def __init__(self, proc):
        self._p = proc
        self.pid = proc.pid

    def __getattr__(self, name):
        return getattr(self._p, name)

    async def detach(self):
        pass


class FakeMachine:
    def __init__(self, sandbox_id: str, template):
        self.sandbox_id = sandbox_id
        self.template = template
        self.files: Dict[str, bytes] = {}
        self.spawned: List[dict] = []
        self.procs: Dict[int, FakeProcess] = {}
        self.deleted = False
        self.forwards: Dict[int, str] = {}
        self.checkpoints: List[str] = []
        self.suspended = False
        self.cold_pause = False  # paused filesystem-only: resume cold-boots
        self.reboots = 0
        self.cold_snapshots: Dict[str, bool] = {}
        self.backend = None
        self.permissive = False
        self.volume_mounted = False
        self.network = None
        self.free_mb = 50_000  # what `df` reports as free on /
        self.lifetime = None  # (seconds, on_expire)
        self._reaper = None
        self.owners: Dict[str, str] = {}
        self.writer = "root"
        self.watchers: Dict[str, List[FsEvent]] = {}
        self.xattrs: Dict[str, dict] = {}
        self.dirs: set = set()  # directories `stat` finds (files are in `files`)
        # What the agent advertises: empty is an old cua-spacesd (command-based
        # fallbacks), {"fs.run_as", "fs.xattr", "process.run_as"} a current one
        # (native file calls, recorded in `native_calls`).
        self.features: frozenset = frozenset()
        self.native_calls: List[tuple] = []

    async def spawn(self, argv, *, env, cwd, user, timeout_ms, stdin, pty=None):
        if self.suspended:
            raise Transport("the VM is suspended")
        self.spawned.append(
            dict(
                argv=argv, env=env, cwd=cwd, user=user, timeout_ms=timeout_ms, stdin=stdin, pty=pty
            )
        )
        if pty:
            p = FakeProcess("forever", None)
            self.procs[p.pid] = p
            return p
        if self.deleted:
            raise NotFound(f"sandbox {self.sandbox_id}")
        if argv[0] == "python3":
            return self._python(argv)
        cmd = argv[-1]
        if cmd.startswith("python3 "):
            import shlex

            return self._python(shlex.split(cmd))
        if cmd == "id -un":
            p = FakeProcess("echo " + ("root" if user == "root" else "cua"), None)
            self.procs[p.pid] = p
            return p
        for prefix in ("sudo mkdir -p ", "mkdir -p "):
            if cmd.startswith(prefix):  # the directory exists afterwards (`stat` finds it)
                import shlex

                self.dirs.update(shlex.split(cmd[len(prefix) :].split(" && ")[0]))
        if cmd.startswith("chown "):
            _, owner, path = cmd.split(" ", 2)
            self.owners[path.strip("'")] = owner.rstrip(":")
            p = FakeProcess("echo ", None)
            self.procs[p.pid] = p
            return p
        if cmd.startswith("df -Pm /"):
            p = FakeProcess("echo " + str(self.free_mb), None)
            self.procs[p.pid] = p
            return p
        if cmd.startswith("mountpoint -q /volume"):
            p = FakeProcess("echo " + ("yes" if self.volume_mounted else "no"), None)
            self.procs[p.pid] = p
            return p
        if self.permissive and cmd.split(" ", 1)[0] not in (
            "echo",
            "fail",
            "chunks",
            "reply",
            "forever",
        ):
            p = FakeProcess("echo", None)  # any other command succeeds
            self.procs[p.pid] = p
            return p
        if cmd == "hang-start":
            await asyncio.sleep(3600)
        if cmd == 'printf %s "$HOME"':
            p = FakeProcess("echo " + ("/root" if user == "root" else "/home/cua"), None)
            p.queue = asyncio.Queue()
            p._put("stdout", b"/root" if user == "root" else b"/home/cua")
            p.done = False
            p._exit(0)
        else:
            p = FakeProcess(cmd, timeout_ms)
        self.procs[p.pid] = p
        return p

    def _python(self, argv):
        """The guest scripts cua-e2b runs: xattr metadata and a resource
        sample."""
        script = argv[2]
        if "setxattr" in script:
            self.xattrs[argv[3]] = {k.lower(): v for k, v in json.loads(argv[4]).items()}
            out = ""
        elif "listxattr" in script and "sys.argv[1:]" in script:
            out = json.dumps({p: self.xattrs[p] for p in argv[3:] if self.xattrs.get(p)}) + "\n"
        elif "listxattr" in script:
            out = json.dumps(self.xattrs.get(argv[3], {})) + "\n"
        elif "/proc/stat" in script:
            out = "2 12.5 1000 400 4096000000 1024000000 512000000\n"
        else:
            out = ""
        p = FakeProcess("echo", None)
        p.queue = asyncio.Queue()
        p.done = False
        if out:
            p._put("stdout", out.encode())
        p._exit(0)
        self.procs[p.pid] = p
        return p

    async def attach(self, pid):
        if pid not in self.procs or self.procs[pid].done:
            raise NotFound(f"process {pid} not found")
        return Attached(self.procs[pid])

    async def list_processes(self):
        return [
            {"pid": p.pid, "config": {"command": "/bin/bash", "args": ["-l", "-c", p.cmd]}}
            for p in self.procs.values()
            if not p.done
        ]

    async def xattr_native(self):
        return "fs.xattr" in self.features

    async def upload(self, path, data, user=None, *, local_path=None, metadata=None):
        if local_path is not None:
            with open(local_path, "rb") as f:
                data = f.read()
        if metadata is not None:
            assert await self.xattr_native(), "metadata handed to an agent without fs.xattr"
            self.native_calls.append(("upload", path, user, dict(metadata)))
            self.xattrs[path] = {k.lower(): v for k, v in metadata.items()}
        self.files[path] = data
        self.owners[path] = user or "cua"
        for root, events in self.watchers.items():
            if path.startswith(root.rstrip("/") + "/"):
                events.append(FsEvent(path=path, type="write"))

    async def download(self, path, user=None):
        if path not in self.files:
            raise NotFound(f"{path}: no such file")
        return self.files[path]

    async def open_download(self, path, user=None):
        data = await self.download(path)
        return FakeDownload(data)

    async def remove(self, path, user=None):
        if path not in self.files:
            raise NotFound(f"{path}: no such file")
        del self.files[path]

    async def _stat_dir(self, path):
        if path in self.dirs:
            return FileStat(
                name=path.rsplit("/", 1)[-1],
                path=path,
                type="dir",
                size=0,
                mode=0o40755,
                modified=None,
                symlink_target=None,
                owner="cua",
                group="cua",
            )

    async def has_user(self, name):
        return name in ("root", "cua", "user", "nobody")

    async def stat(self, path, user=None, *, follow=False):
        if path in self.dirs:
            return await self._stat_dir(path)
        if path not in self.files:
            raise NotFound(f"{path}: no such file")
        return FileStat(
            name=path.rsplit("/", 1)[-1],
            path=path,
            type="file",
            size=len(self.files[path]),
            mode=0o100644,
            modified=None,
            symlink_target=None,
            owner=self.owners.get(path, "cua"),
            group=self.owners.get(path, "cua"),
            metadata=(dict(self.xattrs.get(path) or {}) if await self.xattr_native() else None),
        )

    async def list_dir(self, path, depth, user=None):
        prefix = path.rstrip("/") + "/"
        return [await self.stat(p) for p in self.files if p.startswith(prefix)]

    async def make_dir(self, path, user=None):
        pass

    async def rename(self, old, new, user=None):
        if old not in self.files:
            raise NotFound(f"{old}: no such file")
        self.files[new] = self.files.pop(old)
        return await self.stat(new)

    async def set_egress(self, allow, deny):
        self.egress = {"allow": list(allow), "deny": list(deny)}

    def endpoint(self):
        return "http://10.0.0.5:3211"

    async def signed_url(self, path, method, ttl, user):
        if user not in (None, "cua"):
            from e2b import InvalidArgumentException

            raise InvalidArgumentException("signed URLs are served as the default user")
        return f"http://10.0.0.5:3211/files?path={path}&method={method}&exp={ttl}&sig=x"

    async def metrics(self):
        return {
            "cpu_count": 2,
            "cpu_pct": 12.5,
            "mem_total": 4096000000,
            "mem_used": 1024000000,
            "mem_cache": 512000000,
            "disk_total": 1000,
            "disk_used": 400,
        }

    async def watch(self, path, recursive, user=None):
        self.watchers[path] = []
        return path

    async def watch_events(self, watcher_id):
        events, self.watchers[watcher_id] = self.watchers[watcher_id], []
        return events

    async def unwatch(self, watcher_id):
        self.watchers.pop(watcher_id, None)

    async def set_lifetime(self, seconds, on_expire):
        """Simulates the cua daemon's reaper."""
        self.lifetime = (seconds, on_expire)
        if self._reaper:
            self._reaper.cancel()

        def end():
            if on_expire == "suspend":
                self.suspended = True
            else:
                self.deleted = True

        self._reaper = asyncio.get_running_loop().call_later(seconds, end)
        return True

    async def forward(self, port):
        if port not in self.forwards:
            self.forwards[port] = f"127.0.0.1:{40000 + port}"
        return self.forwards[port]

    async def checkpoint(self, name, cold=False):
        self.checkpoints.append(name)
        self.cold_snapshots[name] = cold
        return name

    async def suspend(self, cold=False):
        self.suspended = True
        self.cold_pause = cold

    async def resume(self, reboot=False):
        was, self.suspended = self.suspended, False
        if was and (reboot or self.cold_pause):
            self.reboots += 1
        if was:
            self.cold_pause = False

    async def reboot(self):
        self.reboots += 1

    async def fork(self, name):
        if self.deleted or self.suspended:
            raise Transport("cannot fork a stopped sandbox")
        child = FakeMachine(name, self.template)
        child.files = dict(self.files)
        child.forked_from = self.sandbox_id
        self.children = getattr(self, "children", []) + [child]
        self.backend.machines[name] = child
        child.backend = self.backend
        return child

    async def delete(self):
        if self.deleted:
            return False
        self.deleted = True
        return True


class FakeDownload:
    def __init__(self, data: bytes, chunk: int = 64 * 1024):
        self._chunks = [data[i : i + chunk] for i in range(0, len(data), chunk)]
        self.cancelled = False

    async def next_chunk(self):
        return self._chunks.pop(0) if self._chunks else None

    def cancel(self):
        self.cancelled = True


class FakeVolume:
    """The `volume_*` calls of the SDK's Spaces, over a dict."""

    def __init__(self):
        self.files: Dict[str, bytes] = {}

    async def volume_write(self, path, content, if_etag, create_only, view):
        if create_only and path in self.files:
            raise CuaError("already exists")
        self.files[path] = bytes(content)
        return types.SimpleNamespace(size=len(content), modified_ms=1_700_000_000_000)

    async def volume_read(self, path, version, view):
        if path not in self.files:
            raise NotFound(path)
        return types.SimpleNamespace(content=self.files[path])

    async def volume_delete(self, path, if_etag, view):
        self.files.pop(path, None)

    async def volume_ls(self, path, view):
        prefix = path.rstrip("/") + "/"
        rows: Dict[str, Any] = {}
        for k, v in self.files.items():
            if k.startswith(prefix):
                head, _, rest = k[len(prefix) :].partition("/")
                if rest:
                    rows[head] = types.SimpleNamespace(
                        name=head, folder=True, size=0, modified_ms=0, path=prefix + head
                    )
                else:
                    rows[head] = types.SimpleNamespace(
                        name=head, folder=False, size=len(v), modified_ms=1_700_000_000_000, path=k
                    )
        if not rows:
            raise NotFound(path)
        return types.SimpleNamespace(entries=list(rows.values()))


class FakeBackend:
    def __init__(self):
        self.machines: Dict[str, FakeMachine] = {}
        self.records: Dict[str, dict] = {}
        self.fail_create: Optional[Exception] = None
        self.permissive = False
        self.volume_mounted = False
        self.templates: Dict[str, dict] = {}
        self.free_mb = 50_000  # what `df` reports as free on / in new machines
        self.volume = FakeVolume()
        self.features: frozenset = frozenset()  # what new machines' agents advertise

    async def client(self):
        return self

    def spaces(self):
        return self.volume

    async def save_template(self, name, record, prebuild=False):
        if record is None:  # like the real backend: None removes the record
            self.templates.pop(name, None)
        else:
            self.templates[name] = record

    def template(self, name):
        return self.templates.get(name)

    async def delete_template(self, name):
        return self.templates.pop(name, None) is not None

    async def create(self, template, *, network=None, egress=None):
        if self.fail_create:
            raise self.fail_create
        m = FakeMachine(f"e2b-{len(self.machines)}", template)
        m.network = network
        m.egress = None
        m.backend = self
        if egress:
            await m.set_egress(egress["allow"], egress["deny"])
        m.permissive = self.permissive
        m.volume_mounted = self.volume_mounted
        m.free_mb = self.free_mb
        m.features = self.features
        self.machines[m.sandbox_id] = m
        self.records[m.sandbox_id] = {"template": template}
        return m

    async def kill(self, sandbox_id):
        m = self.machines.get(sandbox_id)
        if m is None or m.deleted:
            return False
        m.deleted = True
        return True

    def record(self, sandbox_id):
        return dict(self.records.get(sandbox_id, {}))

    def annotate(self, sandbox_id, **fields):
        self.records.setdefault(sandbox_id, {}).update(fields)

    def forget(self, sandbox_id):
        self.records.pop(sandbox_id, None)

    async def list(self):
        return [
            MachineInfo(
                sandbox_id=i,
                template=self.records[i].get("template"),
                started_at=None,
                state="paused" if m.suspended else "running",
                extra=self.records[i],
            )
            for i, m in self.machines.items()
            if not m.deleted and i in self.records
        ]

    async def state(self, sandbox_id):
        m = self.machines.get(sandbox_id)
        if m is None or m.deleted:
            return None
        return "paused" if m.suspended else "running"

    async def list_snapshots(self):
        return [
            {"name": n, "source": i, "created": f"2026-01-0{k}"}
            for k, (i, m) in enumerate(self.machines.items(), 1)
            for n in m.cold_snapshots
        ]

    async def delete_snapshot(self, name):
        for m in self.machines.values():
            if name in m.cold_snapshots:
                del m.cold_snapshots[name]
                return True
        return False

    async def connect(self, sandbox_id, reboot=False):
        if sandbox_id not in self.machines:
            raise NotFound(sandbox_id)
        m = self.machines[sandbox_id]
        if m.suspended:  # the real backend resumes a paused sandbox here
            await m.resume(reboot)
        return m

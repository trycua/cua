"""File calls: e2b's root-run semantics over a service that acts as one user
(`CuaMachine` against a model of cua-spacesd) and the e2b-shaped results and
errors of `sandbox.files` (over the in-memory backend)."""

from __future__ import annotations

import json
import posixpath
import sys
import types
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Dict, List, Optional

import pytest
from e2b import (
    AsyncSandbox,
    FileNotFoundException,
    InvalidArgumentException,
    SandboxException,
)
from e2b._backend import CuaMachine, FileStat, set_backend
from e2b.exceptions import AuthenticationException
from fake_backend import FakeBackend

# ── a model of cua-spacesd's file service and of the guest's commands ──────


class CuaError(Exception):
    pass


class PermissionDenied(CuaError):
    pass


class NotFound(CuaError):
    pass


@dataclass
class _Cmd:
    program: str
    args: List[str] = field(default_factory=list)
    user: Optional[str] = None
    timeout_ms: Optional[int] = None


@dataclass
class _Opts:
    create_parents: bool = False
    user: Optional[str] = None
    metadata: Optional[Dict[str, str]] = None
    xattr_namespace: Optional[str] = None


class _Exit:
    def __init__(self, code):
        self.code = code


class _Out:
    def __init__(self, stdout=b"", stderr=b"", code=0):
        self.stdout, self.stderr, self.exit = stdout, stderr, _Exit(code)


class _Dl:
    def __init__(self, data: bytes):
        self.data = data
        self.cancelled = False

    async def next_chunk(self):
        d, self.data = self.data, b""
        return d or None

    def cancel(self):
        self.cancelled = True


class Guest:
    """Files as (type, data, owner, target); the file service runs as `cua`
    and is refused anything below a path in `locked`."""

    def __init__(self):
        self.nodes: Dict[str, dict] = {"/": {"type": "dir", "owner": "root"}}
        self.locked: set = set()
        self.users = {"root", "cua", "nobody"}
        self.ran: List[_Cmd] = []
        self.add("/tmp", "dir", owner="root")

    def add(self, path, type="file", data=b"", owner="cua", target=None):
        self.nodes[path] = {"type": type, "data": data, "owner": owner, "target": target}

    def _denied(self, path):
        return any(path == p or path.startswith(p.rstrip("/") + "/") for p in self.locked)

    def _check(self, path):
        if self._denied(path):
            raise PermissionDenied(f"permission denied: {path}")

    def _follow(self, path):
        for _ in range(40):
            n = self.nodes.get(path)
            if n is None or n["type"] != "symlink":
                return path
            path = posixpath.join(posixpath.dirname(path), n["target"])
        return path

    def _mkparents(self, path, owner):
        d = posixpath.dirname(path)
        if d not in self.nodes:
            self._mkparents(d, owner)
            self.add(d, "dir", owner=owner)

    # the SDK's env

    async def upload(self, path, data, opts):
        self._check(path)
        self._put(path, data, "cua")

    def _put(self, path, data, owner):
        self._mkparents(path, owner)
        self.add(path, "file", bytes(data), owner)

    async def upload_file(self, local, path, opts):
        with open(local, "rb") as f:
            await self.upload(path, f.read(), opts)

    async def download(self, path):
        self._check(path)
        return self.nodes[self._follow(path)]["data"]

    async def open_download(self, path, _):
        self._check(path)
        if path not in self.nodes:
            raise NotFound(path)
        return _Dl(self.nodes[self._follow(path)]["data"])

    async def remove(self, path, recursive):
        self._check(path)
        self.nodes.pop(path, None)

    async def make_dir(self, path):
        self._check(path)
        self._mkparents(path + "/x", "cua")

    def _entry(self, path, follow=False):
        real = self._follow(path) if follow else path
        n = self.nodes.get(real)
        if n is None:
            raise NotFound(path)
        t = {"file": "FILE_TYPE_FILE", "dir": "FILE_TYPE_DIRECTORY", "symlink": "FILE_TYPE_SYMLINK"}
        e = {
            "name": posixpath.basename(real),
            "path": real,
            "type": t[n["type"]],
            "size": len(n.get("data", b"")),
            "mode": 0o644,
            "owner": n["owner"],
            "group": n["owner"],
        }
        if n["type"] == "symlink":
            e["symlinkTarget"] = n["target"]
        return e

    async def call_json(self, method, request):
        r = json.loads(request)
        self._check(r["path"] if "path" in r else r["source"])
        if method.endswith("/Stat"):
            return json.dumps({"entry": self._entry(r["path"], not r.get("noFollowSymlinks"))})
        if method.endswith("/ListDir"):
            p = r["path"].rstrip("/") + "/"
            kids = sorted(k for k in self.nodes if k.startswith(p) and "/" not in k[len(p) :])
            return json.dumps({"entries": [self._entry(k) for k in kids if k != "/"]})
        if method.endswith("/Move"):
            if r["source"] not in self.nodes:
                raise NotFound(r["source"])
            self._mkparents(r["destination"], "cua")
            self.nodes[r["destination"]] = self.nodes.pop(r["source"])
            return json.dumps({"entry": self._entry(r["destination"])})
        raise AssertionError(method)

    async def run(self, cmd):
        self.ran.append(cmd)
        a = cmd.args
        if cmd.program == "id":
            if a == ["-un"]:
                return _Out(b"cua\n")
            return _Out(code=0 if a[-1] in self.users else 1)
        if cmd.program == "chown":
            owner = a[0].rstrip(":")
            for p in a[2:]:
                self.nodes[p]["owner"] = owner
            return _Out()
        if cmd.program == "cat":
            return _Out(self.nodes[self._follow(a[1])]["data"])
        if cmd.program == "rm":
            self.nodes.pop(a[2], None)
            return _Out()
        if cmd.program == "mkdir":
            self._mkparents(a[2] + "/x", "root")
            return _Out()
        if cmd.program == "sh":
            script, args = a[1], a[3:]
            if 'mv -f -- "$2" "$1"' in script:  # staged upload
                path, stage = args
                self._mkparents(path, "root")
                self.nodes[path] = self.nodes.pop(stage) | {"owner": "root"}
            elif "cp --" in script:  # staged download
                path, stage, me = args
                self.nodes[stage] = dict(self.nodes[path], owner=me)
            elif 'mv -f -- "$1" "$2"' in script:
                old, new = args
                self._mkparents(new, "root")
                self.nodes[new] = self.nodes.pop(old)
            else:
                raise AssertionError(script)
            return _Out()
        if cmd.program == "python3":
            path, mode = a[2], a[3]
            if mode == "ls":
                p = path.rstrip("/") + "/"
                kids = sorted(k for k in self.nodes if k.startswith(p) and "/" not in k[len(p) :])
                return _Out(json.dumps([self._entry(k) for k in kids]).encode())
            return _Out(json.dumps(self._entry(path, mode == "stat")).encode())
        raise AssertionError(cmd)


NATIVE = ("fs.run_as", "fs.xattr", "process.run_as", "files.multipart")


class NativeGuest(Guest):
    """A current cua-spacesd with a current SDK: it advertises `fs.run_as`
    and `fs.xattr`, the typed file calls take `user=` / `xattr_namespace=`
    and act as that user (the default: `cua`; `root` is never refused). The
    helper commands of the old path are an error here."""

    HELPERS = ("cat", "rm", "mkdir", "sh", "python3")

    def __init__(self, features=NATIVE):
        super().__init__()
        self.features = features
        self.calls: List[tuple] = []
        self.capability_reads = 0
        self.xattrs: Dict[str, dict] = {}

    async def capabilities(self):
        self.capability_reads += 1
        return types.SimpleNamespace(
            features=[
                types.SimpleNamespace(name=n, supported=True, limitation=None, attributes={})
                for n in self.features
            ]
        )

    def _as(self, path, user):
        if (user or "cua") != "root":
            self._check(path)

    def _fe(self, path, follow=False, namespace=None):
        e = self._entry(path, follow)
        kind = {
            "FILE_TYPE_FILE": "file",
            "FILE_TYPE_DIRECTORY": "directory",
            "FILE_TYPE_SYMLINK": "symlink",
        }[e["type"]]
        return types.SimpleNamespace(
            name=e["name"],
            path=e["path"],
            kind=kind,
            size=e["size"],
            mode=e["mode"],
            modified_ms=None,
            symlink_target=e.get("symlinkTarget"),
            owner=e["owner"],
            group=e["group"],
            metadata=dict(self.xattrs.get(e["path"], {})) if namespace == "user.e2b." else {},
        )

    async def upload(self, path, data, opts):
        self.calls.append(("upload", path, opts.user))
        self._as(path, opts.user)
        self._put(path, data, opts.user or "cua")
        if opts.metadata:
            assert opts.xattr_namespace == "user.e2b."
            self.xattrs[path] = dict(opts.metadata)

    async def download(self, path, user=None):
        self.calls.append(("download", path, user))
        self._as(path, user)
        return self.nodes[self._follow(path)]["data"]

    async def open_download(self, path, chunk_size=None, user=None):
        self.calls.append(("open_download", path, user))
        self._as(path, user)
        if path not in self.nodes:
            raise NotFound(path)
        return _Dl(self.nodes[self._follow(path)]["data"])

    async def remove(self, path, recursive, user=None):
        self.calls.append(("remove", path, user))
        self._as(path, user)
        self.nodes.pop(path, None)

    async def make_dir(self, path, user=None):
        self.calls.append(("make_dir", path, user))
        self._as(path, user)
        self._mkparents(path + "/x", user or "cua")
        return self._fe(path)

    async def stat(self, path, user=None, xattr_namespace=None, no_follow=False):
        self.calls.append(("stat", path, user))
        self._as(path, user)
        return self._fe(path, not no_follow, xattr_namespace)

    async def list_dir(self, path, depth, user=None, xattr_namespace=None):
        self.calls.append(("list_dir", path, user))
        self._as(path, user)
        p = path.rstrip("/") + "/"
        kids = sorted(k for k in self.nodes if k.startswith(p) and "/" not in k[len(p) :])
        return [self._fe(k, False, xattr_namespace) for k in kids if k != "/"]

    async def move_path(
        self, source, destination, overwrite=False, create_parents=False, user=None
    ):
        self.calls.append(("move_path", source, destination, user, overwrite, create_parents))
        self._as(source, user)
        if source not in self.nodes:
            raise NotFound(source)
        self._mkparents(destination, user or "cua")
        self.nodes[destination] = self.nodes.pop(source)
        return self._fe(destination)

    async def call_json(self, method, request, user=None):
        self.calls.append(("call_json", method, json.loads(request), user))
        return json.dumps({"urlPath": "/files?path=%2Ftmp%2Fa&sig=s"})

    def endpoint(self):
        return "http://10.0.0.5:3211"

    async def run(self, cmd):
        assert cmd.program not in self.HELPERS, f"helper command on a native agent: {cmd}"
        return await super().run(cmd)


@pytest.fixture(params=["old", "native"])
def guest(request, monkeypatch):
    """The file semantics hold on an old agent (command fallbacks) and a
    current one (native `user=` calls)."""
    cua = types.ModuleType("cua")
    cua.SpacesdCommand = _Cmd
    cua.UploadOptions = _Opts
    monkeypatch.setitem(sys.modules, "cua", cua)
    return Guest() if request.param == "old" else NativeGuest()


def machine(guest) -> CuaMachine:
    return CuaMachine(None, "sbx", guest)


# ── CuaMachine: what e2b's root-run file calls do over a service acting as cua ──


async def test_write_into_a_locked_dir_is_done_as_root_and_given_to_the_user(guest):
    guest.add("/locked", "dir", owner="root")
    guest.locked.add("/locked")
    m = machine(guest)
    await m.upload("/locked/new/x.txt", b"hi", None)
    assert guest.nodes["/locked/new/x.txt"]["data"] == b"hi"
    assert guest.nodes["/locked/new/x.txt"]["owner"] == "cua"
    assert guest.nodes["/locked/new"]["owner"] == "cua"  # the parent it created
    assert not [p for p in guest.nodes if ".cua-e2b-" in p]  # staging file moved


async def test_write_for_another_user_gives_files_and_new_dirs_to_that_user(guest):
    m = machine(guest)
    await m.upload("/tmp/a/b/f.txt", b"x", "root")
    assert [guest.nodes[p]["owner"] for p in ("/tmp/a", "/tmp/a/b", "/tmp/a/b/f.txt")] == [
        "root"
    ] * 3
    assert guest.nodes["/tmp"]["owner"] == "root"  # untouched
    await m.upload("/tmp/a/b/g.txt", b"x", "nobody")
    assert guest.nodes["/tmp/a/b"]["owner"] == "root"  # an existing dir is left alone
    assert guest.nodes["/tmp/a/b/g.txt"]["owner"] == "nobody"


async def test_write_follows_a_symbolic_link(guest):
    guest.add("/tmp/t", data=b"old")
    guest.add("/tmp/l", "symlink", target="t")
    await machine(guest).upload("/tmp/l", b"new", None)
    assert guest.nodes["/tmp/l"]["type"] == "symlink"
    assert guest.nodes["/tmp/t"]["data"] == b"new"


async def test_write_streams_a_local_file(guest, tmp_path):
    f = tmp_path / "src.bin"
    f.write_bytes(b"\x00\x01")
    await machine(guest).upload("/tmp/dst", b"", None, local_path=str(f))
    assert guest.nodes["/tmp/dst"]["data"] == b"\x00\x01"


async def test_reads_removes_and_stats_run_as_root_when_refused(guest):
    guest.add("/secret", data=b"s", owner="root")
    guest.locked.add("/secret")
    m = machine(guest)
    assert await m.download("/secret", "nobody") == b"s"
    d = await m.open_download("/secret", None)
    assert await d.next_chunk() == b"s" and await d.next_chunk() is None
    assert (await m.stat("/secret")).owner == "root"
    await m.remove("/secret", None)
    assert "/secret" not in guest.nodes


async def test_make_dir_as_root_after_a_refusal_is_given_to_the_user(guest):
    guest.locked.add("/etc")
    guest.add("/etc", "dir", owner="root")
    m = machine(guest)
    await m.make_dir("/etc/x/y", None)
    assert guest.nodes["/etc/x/y"]["owner"] == "cua"
    assert guest.nodes["/etc/x"]["owner"] == "cua"
    assert guest.nodes["/etc"]["owner"] == "root"


async def test_stat_does_not_follow_links_and_reports_the_resolved_target(guest):
    guest.add("/tmp/t")
    guest.add("/tmp/l1", "symlink", target="t")
    guest.add("/tmp/l2", "symlink", target="/tmp/l1")
    guest.add("/tmp/dead", "symlink", target="/no/such")
    m = machine(guest)
    st = await m.stat("/tmp/l2")
    assert (st.type, st.symlink_target) == ("symlink", "/tmp/t")
    assert (await m.stat("/tmp/l2", follow=True)).type == "file"
    dead = await m.stat("/tmp/dead")
    assert (dead.type, dead.symlink_target) == ("symlink", "/tmp/dead")


async def test_listing_leaves_out_links_whose_target_is_gone(guest):
    guest.add("/tmp/d", "dir")
    guest.add("/tmp/d/f")
    guest.add("/tmp/d/ok", "symlink", target="f")
    guest.add("/tmp/d/dead", "symlink", target="/no/such")
    got = {e.name: e for e in await machine(guest).list_dir("/tmp/d", 1)}
    assert sorted(got) == ["f", "ok"]
    assert got["ok"].symlink_target == "/tmp/d/f"


async def test_unknown_users_are_found_out_once(guest):
    m = machine(guest)
    assert await m.has_user("nobody") and not await m.has_user("nosuchuser")
    n = len(guest.ran)
    assert await m.has_user("nobody")
    assert len(guest.ran) == n


# ── sandbox.files over the in-memory backend ────────────────────────────────


@pytest.fixture
def backend(monkeypatch):
    monkeypatch.delenv("CUA_E2B_TEMPLATES", raising=False)
    b = FakeBackend()
    set_backend(b)
    yield b
    set_backend(None)


async def _sb(backend) -> AsyncSandbox:
    return await AsyncSandbox.create(template="world-1")


async def test_unknown_user_is_an_authentication_error_on_every_call(backend):
    sb = await _sb(backend)
    await sb.files.write("/tmp/f", "x")
    for call in (
        sb.files.read("/tmp/f", user="nosuchuser"),
        sb.files.write("/tmp/f", "y", user="nosuchuser"),
        sb.files.list("/tmp", user="nosuchuser"),
        sb.files.exists("/tmp/f", user="nosuchuser"),
        sb.files.get_info("/tmp/f", user="nosuchuser"),
        sb.files.remove("/tmp/f", user="nosuchuser"),
        sb.files.make_dir("/tmp/d", user="nosuchuser"),
        sb.files.rename("/tmp/f", "/tmp/g", user="nosuchuser"),
    ):
        with pytest.raises(AuthenticationException) as info:
            await call
        assert str(info.value) == (
            "error looking up user 'nosuchuser': user: unknown user nosuchuser"
        )


async def test_permissions_begin_with_the_file_type_character(backend):
    sb = await _sb(backend)
    await sb.files.write("/tmp/f", "x")
    assert (await sb.files.get_info("/tmp/f")).permissions == "-rw-r--r--"


async def test_read_in_an_unknown_format_reads_and_returns_nothing(backend):
    sb = await _sb(backend)
    await sb.files.write("/tmp/f", "x")
    assert await sb.files.read("/tmp/f", format="nope") is None
    with pytest.raises(FileNotFoundException):
        await sb.files.read("/tmp/missing", format="nope")


async def test_duplicate_paths_in_one_upload_write_the_first_and_refuse(backend):
    sb = await _sb(backend)
    with pytest.raises(InvalidArgumentException) as info:
        await sb.files.write_files(
            [
                {"path": "/tmp/a", "data": "1"},
                {"path": "/tmp/dup", "data": "first"},
                {"path": "/tmp/dup", "data": "second"},
                {"path": "/tmp/after", "data": "3"},
            ]
        )
    assert str(info.value) == (
        "you cannot upload multiple files to the same path '/tmp/dup' in one upload "
        "request, only the first specified file was uploaded"
    )
    assert await sb.files.read("/tmp/dup") == "first"
    assert await sb.files.read("/tmp/a") == "1"
    assert not await sb.files.exists("/tmp/after")


async def test_duplicate_paths_are_not_refused_on_the_octet_stream_path(backend):
    sb = await _sb(backend)
    out = await sb.files.write_files(
        [{"path": "/tmp/d", "data": "1"}, {"path": "/tmp/d", "data": "2"}], use_octet_stream=True
    )
    assert len(out) == 2


async def test_metadata_limits_are_envds(backend):
    sb = await _sb(backend)
    with pytest.raises(InvalidArgumentException) as info:
        await sb.files.write("/tmp/m", "x", metadata={"K" * 247: "v"})
    assert str(info.value) == f'invalid metadata: metadata key "{"k" * 247}" exceeds 246 bytes'
    with pytest.raises(InvalidArgumentException) as info:
        await sb.files.write("/tmp/m", "x", metadata={"k": "v" * 5000})
    assert str(info.value) == "invalid metadata: total metadata size 5010 exceeds 4096 bytes"
    await sb.files.write("/tmp/m", "x", metadata={"k" * 246: "v"})


async def test_listing_and_rename_carry_the_metadata(backend):
    sb = await _sb(backend)
    await sb.files.write("/d/a", "x", metadata={"Kind": "in"})
    await sb.files.write("/d/b", "x")
    got = {e.name: e.metadata for e in await sb.files.list("/d")}
    assert got == {"a": {"kind": "in"}, "b": None}


def _dirs(backend, sb, *dirs):
    """Marks `dirs` as directories in the sandbox's in-memory files."""
    m = backend.machines[sb.sandbox_id]
    plain = m.stat

    async def stat(path, user=None, *, follow=False):
        if path in dirs:
            return FileStat(posixpath.basename(path), path, "dir", 0, 0o755, None, None)
        return await plain(path, user)

    moved = []

    async def rename(old, new, user=None):
        moved.append((old, new))
        m.files[new] = m.files.pop(old)
        return await stat(new)

    m.stat, m.rename = stat, rename
    return moved


async def test_rename_follows_go_os_rename(backend):
    sb = await _sb(backend)
    await sb.files.write("/t/file", "f")
    await sb.files.write("/t/other", "o")
    moved = _dirs(backend, sb, "/t/dir", "/t/dir2")

    info = await sb.files.rename("/t/file", "/t/other")  # file over file: replaced
    assert info.path == "/t/other" and moved == [("/t/file", "/t/other")]

    await sb.files.write("/t/file", "f")
    with pytest.raises(SandboxException) as e:  # file over a directory
        await sb.files.rename("/t/file", "/t/dir")
    assert str(e.value) == "Code.INTERNAL: error renaming: rename /t/file /t/dir: file exists"
    with pytest.raises(SandboxException) as e:  # a directory onto itself
        await sb.files.rename("/t/dir", "/t/dir")
    assert str(e.value).endswith("rename /t/dir /t/dir: file exists")
    with pytest.raises(SandboxException) as e:  # a directory over a file
        await sb.files.rename("/t/dir", "/t/other")
    assert str(e.value).endswith("rename /t/dir /t/other: not a directory")
    assert len(moved) == 1
    with pytest.raises(FileNotFoundException) as e:  # a missing source over a directory
        await sb.files.rename("/t/none", "/t/dir2")
    assert str(e.value) == (
        "source file not found: rename /t/none /t/dir2: no such file or directory"
    )
    info = await sb.files.rename("/t/other", "/t/other")  # a file onto itself: nothing to do
    assert info.path == "/t/other" and len(moved) == 1


async def test_a_file_in_a_sticky_directory_can_be_rewritten(backend):
    sb = await _sb(backend)
    await sb.files.write("/tmp/again", "a much longer first content")
    await sb.files.write("/tmp/again", "short")
    assert await sb.files.read("/tmp/again") == "short"


async def test_a_directory_reports_the_size_its_file_system_gives(backend):
    sb = await _sb(backend)
    m = backend.machines[sb.sandbox_id]

    async def stat(path, user=None, *, follow=False):
        return FileStat("d", path, "dir", 4096, 0o755, None, None)

    m.stat = stat
    assert (await sb.files.get_info("/tmp/d")).size == 4096


async def test_a_refused_removal_is_an_internal_error_with_the_guests_words(backend):
    sb = await _sb(backend)
    m = backend.machines[sb.sandbox_id]

    async def remove(path, user=None):
        raise PermissionError("rm: cannot remove '/proc/1/x': Operation not permitted")

    m.remove = remove
    with pytest.raises(SandboxException) as info:
        await sb.files.remove("/proc/1")
    assert str(info.value) == (
        "Code.INTERNAL: error removing file or directory: "
        "rm: cannot remove '/proc/1/x': Operation not permitted"
    )


# ── native file calls when the agent advertises fs.run_as / fs.xattr ─────────


@pytest.fixture
def native(monkeypatch):
    cua = types.ModuleType("cua")
    cua.SpacesdCommand = _Cmd
    cua.UploadOptions = _Opts
    monkeypatch.setitem(sys.modules, "cua", cua)
    return NativeGuest()


async def test_features_are_read_once_per_machine(native):
    m = machine(native)
    for _ in range(3):
        await m.stat("/tmp")
        await m.upload("/tmp/f", b"x", None)
    assert native.capability_reads == 1


async def test_root_retry_is_the_same_call_as_root_and_runs_no_helper(native):
    native.add("/secret", data=b"s", owner="root")
    native.add("/locked", "dir", owner="root")
    native.locked.update({"/secret", "/locked"})
    m = machine(native)
    assert await m.download("/secret", None) == b"s"
    d = await m.open_download("/secret", None)
    assert await d.next_chunk() == b"s"
    assert (await m.stat("/secret")).owner == "root"
    await m.upload("/locked/new/x.txt", b"hi", None)
    await m.make_dir("/locked/d/e", None)
    await m.rename("/locked/new/x.txt", "/locked/y/z.txt")
    await m.remove("/secret", None)
    assert [c for c in native.ran if c.program != "chown" and c.program != "id"] == []
    assert not [p for p in native.nodes if ".cua-e2b-" in p]
    assert ("remove", "/secret", "root") in native.calls
    assert ("download", "/secret", "root") in native.calls
    assert native.nodes["/locked/new"]["owner"] == "cua"  # given to the user, as before
    assert native.nodes["/locked/d/e"]["owner"] == "cua"
    assert native.nodes["/locked/y"]["owner"] == "cua"  # the parent the move created


async def test_a_call_for_another_user_acts_as_that_user(native):
    native.add("/tmp/box", "dir", owner="nobody")
    m = machine(native)
    await m.upload("/tmp/box/a/b.txt", b"x", "nobody")
    await m.make_dir("/tmp/box/c", "nobody")
    assert ("upload", "/tmp/box/a/b.txt", "nobody") in native.calls
    assert ("make_dir", "/tmp/box/c", "nobody") in native.calls
    # created by the agent as that user: no chown, no sudo, no su
    assert native.nodes["/tmp/box/a/b.txt"]["owner"] == "nobody"
    assert not [c for c in native.ran if c.program in ("chown", "sudo", "su")]


async def test_default_user_calls_pass_no_user(native):
    m = machine(native)
    await m.upload("/tmp/q", b"x", None)
    await m.upload("/tmp/q2", b"x", "cua")  # the agent's own user: same as none
    assert [c[2] for c in native.calls if c[0] == "upload"] == [None, None]


async def test_signed_url_for_another_user_uses_create_signed_url_run_as(native):
    m = machine(native)
    url = await m.signed_url("/tmp/a", "PUT", 60, "root")
    assert url.endswith("/files?path=%2Ftmp%2Fa&sig=s")
    _, method, request, user = [c for c in native.calls if c[0] == "call_json"][0]
    assert method.endswith("CreateSignedUrl") and user == "root"
    assert request["createParents"] is True and "runAs" not in request
    await m.signed_url("/tmp/a", "GET", 60, None)
    assert [c for c in native.calls if c[0] == "call_json"][1][3] is None


async def test_fs_run_as_without_an_sdk_that_takes_user_keeps_the_fallbacks(monkeypatch):
    """The advertised feature is not enough: the SDK must take `user=`."""

    class OldSdk(NativeGuest):
        async def stat(self, path, no_follow=False):  # pre-R2 signature
            return await Guest.call_json(
                self, "/Stat", json.dumps({"path": path, "noFollowSymlinks": no_follow})
            )

    cua = types.ModuleType("cua")
    cua.SpacesdCommand, cua.UploadOptions = _Cmd, _Opts
    monkeypatch.setitem(sys.modules, "cua", cua)
    g = OldSdk()
    assert not await machine(g).run_as_native()
    assert not await machine(g).xattr_native()


async def test_old_agents_advertise_nothing_and_use_helpers(guest):
    if isinstance(guest, NativeGuest):
        pytest.skip("native agent")
    m = machine(guest)
    assert not await m.run_as_native() and not await m.xattr_native()
    guest.add("/secret", data=b"s", owner="root")
    guest.locked.add("/secret")
    await m.download("/secret", None)
    assert any(c.program == "cat" for c in guest.ran)


async def test_only_xattr_advertised_reads_metadata_natively(monkeypatch):
    cua = types.ModuleType("cua")
    cua.SpacesdCommand, cua.UploadOptions = _Cmd, _Opts
    monkeypatch.setitem(sys.modules, "cua", cua)
    g = NativeGuest(features=("fs.xattr",))
    m = machine(g)
    assert await m.xattr_native() and not await m.run_as_native()
    await m.upload("/tmp/m", b"x", None, metadata={"Kind": "in"})
    assert g.xattrs["/tmp/m"] == {"kind": "in"}
    assert (await m.stat("/tmp/m")).metadata == {"kind": "in"}
    assert (await m.list_dir("/tmp", 1))[0].metadata == {"kind": "in"}


# sandbox.files over the in-memory backend, on an old and a current agent


@pytest.fixture(params=["old", "native"])
def agent(request, backend):
    backend.features = frozenset(NATIVE) if request.param == "native" else frozenset()
    return request.param


async def test_metadata_round_trips_on_both_agents(backend, agent):
    sb = await _sb(backend)
    m = backend.machines[sb.sandbox_id]
    await sb.files.write("/d/a", "x", metadata={"Kind": "in"})
    await sb.files.write("/d/b", "x")
    info = await sb.files.get_info("/d/a")
    assert info.metadata == {"kind": "in"}
    got = {e.name: e.metadata for e in await sb.files.list("/d")}
    assert got == {"a": {"kind": "in"}, "b": None}
    python = [s for s in m.spawned if s["argv"][:2] == ["python3", "-c"]]
    if agent == "native":
        assert python == []  # no helper scripts: the write and the stats carry it
        assert m.native_calls == [("upload", "/d/a", None, {"kind": "in"})]
    else:
        assert python and m.native_calls == []


async def test_watchers_open_as_the_user_on_a_native_agent(native, monkeypatch):
    async def call_json(method, request, user=None):
        native.calls.append(("call_json", method, json.loads(request), user))
        return json.dumps({"watcherId": "w1"})

    monkeypatch.setattr(native, "call_json", call_json)
    m = machine(native)
    assert await m.watch("/tmp", True, "nobody") == "w1"
    assert native.calls[-1][3] == "nobody" and native.calls[-1][2]["recursive"] is True
    await m.watch("/tmp", False, None)
    assert native.calls[-1][3] is None

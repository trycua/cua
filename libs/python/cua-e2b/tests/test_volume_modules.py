"""The volume area of the e2b module layout and every option of the volume
calls, over the in-memory backend and its fake Cua Volume (no VMs, no host
processes)."""

from __future__ import annotations

import datetime
import io
import json
import os

import e2b
import pytest
from e2b._backend import set_backend
from e2b.exceptions import (
    InvalidArgumentException,
    NotFoundException,
    VolumeException,
    VolumeNotFoundException,
    VolumePathNotFoundException,
)
from e2b.volume import connection_config as vcc
from e2b.volume.connection_config import VolumeApiParams, VolumeConnectionConfig
from e2b.volume.types import (
    VolumeAndToken,
    VolumeEntryStat,
    VolumeEntryStatType,
    VolumeFileType,
    VolumeInfo,
)
from e2b.volume.utils import DualMethod, RawEntryStat, convert_volume_entry_stat
from e2b.volume.volume_async import MARKER, AsyncVolume, mount_source
from e2b.volume.volume_sync import Volume
from fake_backend import FakeBackend


@pytest.fixture
def backend(monkeypatch):
    for k in ("E2B_DOMAIN", "E2B_DEBUG", "E2B_VOLUME_API_URL", "E2B_HTTP_VERSION"):
        monkeypatch.delenv(k, raising=False)
    b = FakeBackend()
    set_backend(b)
    yield b
    set_backend(None)


@pytest.fixture
async def vol(backend) -> AsyncVolume:
    return await AsyncVolume.create("data")


def names(entries) -> list:
    return [e.path for e in entries]


# ── layout ─────────────────────────────────────────────────────────────────


def test_every_class_lives_in_the_module_e2b_has_it_in():
    assert AsyncVolume.__module__ == "e2b.volume.volume_async"
    assert Volume.__module__ == "e2b.volume.volume_sync"
    assert VolumeInfo.__module__ == "e2b.volume.types"
    assert VolumeAndToken.__module__ == "e2b.volume.types"
    assert VolumeEntryStat.__module__ == "e2b.volume.types"
    assert VolumeConnectionConfig.__module__ == "e2b.volume.connection_config"
    assert DualMethod.__module__ == "e2b.volume.utils"
    assert VolumeFileType is VolumeEntryStatType
    assert issubclass(VolumeAndToken, VolumeInfo)
    from e2b.connection_config import ClientFactory

    assert issubclass(AsyncVolume, ClientFactory) and issubclass(Volume, ClientFactory)
    # re-exports keep every old import path
    assert e2b.AsyncVolume is AsyncVolume and e2b.Volume is Volume
    assert e2b.volume.AsyncVolume is AsyncVolume and e2b.volume.Volume is Volume
    assert e2b.VolumeNotFoundException is VolumeNotFoundException
    assert e2b.volume.VolumePathNotFoundException is VolumePathNotFoundException
    assert issubclass(VolumeNotFoundException, NotFoundException)
    assert issubclass(VolumePathNotFoundException, NotFoundException)
    assert e2b.VolumeConnectionConfig is VolumeConnectionConfig
    assert e2b.VolumeApiParams is VolumeApiParams
    assert mount_source("data") == "/volume/e2b-volumes/data"


def test_entry_type_values_and_strings():
    assert [t.value for t in VolumeEntryStatType] == ["directory", "file", "symlink", "unknown"]
    assert str(VolumeEntryStatType.FILE) == "file"


# ── connection settings ────────────────────────────────────────────────────


def test_connection_config_resolves_options_and_environment(monkeypatch):
    c = VolumeConnectionConfig()
    assert (c.domain, c.debug, c.token, c.http_version) == ("e2b.app", False, None, "2")
    assert c.api_url == "https://api.e2b.app" and c.request_timeout == 60.0
    assert c.headers["User-Agent"].startswith("cua-e2b/")
    monkeypatch.setenv("E2B_DOMAIN", "env.example")
    monkeypatch.setenv("E2B_DEBUG", "TRUE")
    monkeypatch.setenv("E2B_HTTP_VERSION", "1.1")
    c = VolumeConnectionConfig()
    assert (c.domain, c.debug, c.http_version) == ("env.example", True, "1.1")
    assert c.api_url == "http://localhost:8080"
    monkeypatch.setenv("E2B_VOLUME_API_URL", "http://vol.test")
    assert VolumeConnectionConfig().api_url == "http://vol.test"
    c = VolumeConnectionConfig(
        domain="d.io",
        debug=False,
        token="t",
        api_url="http://x",
        request_timeout=5,
        headers={"X": "1"},
        http_version="2",
    )
    assert (c.domain, c.debug, c.token, c.access_token) == ("d.io", False, "t", "t")
    assert c.api_url == "http://x" and c.request_timeout == 5 and c.headers["X"] == "1"
    assert c.http_version == "2"


def test_request_timeout_zero_means_no_timeout():
    assert VolumeConnectionConfig._get_request_timeout(60, 0) is None
    assert VolumeConnectionConfig._get_request_timeout(60, 3) == 3
    assert VolumeConnectionConfig._get_request_timeout(60, None) == 60
    assert VolumeConnectionConfig(request_timeout=0).request_timeout is None
    assert VolumeConnectionConfig(request_timeout=0).get_request_timeout(7) == 7


def test_connection_config_lays_per_call_options_over_its_own():
    c = VolumeConnectionConfig(domain="a", token="t", headers={"A": "1"}, proxy="http://p")
    base = c.get_api_params()
    assert (base["domain"], base["token"], base["proxy"]) == ("a", "t", "http://p")
    over = c.get_api_params(domain="b", headers={"B": "2"}, request_timeout=9, debug=True)
    assert over["domain"] == "b" and over["debug"] is True and over["request_timeout"] == 9
    assert over["headers"]["A"] == "1" and over["headers"]["B"] == "2"
    assert c.headers == {"A": "1", "User-Agent": c.headers["User-Agent"]}  # not mutated
    assert vcc.REQUEST_TIMEOUT == 60 and vcc.FILE_TIMEOUT == 3600 and vcc.READ_TIMEOUT == 60


# ── helpers ────────────────────────────────────────────────────────────────


def raw(**kw) -> RawEntryStat:
    d = dict(
        name="f", type_=VolumeEntryStatType.FILE, path="/f", size=1, mode=0o644, uid=1, gid=2,
        atime=datetime.datetime(2026, 1, 1), mtime=datetime.datetime(2026, 1, 2),
        ctime=datetime.datetime(2026, 1, 3, tzinfo=datetime.timezone.utc),
    )  # fmt: skip
    d.update(kw)
    return RawEntryStat(**d)


def test_convert_volume_entry_stat_marks_times_utc_and_maps_the_target():
    s = convert_volume_entry_stat(raw(target="/t", type_=VolumeEntryStatType.SYMLINK))
    assert isinstance(s, VolumeEntryStat) and s.type is VolumeEntryStatType.SYMLINK
    assert s.target == "/t" and (s.uid, s.gid, s.mode, s.size) == (1, 2, 0o644, 1)
    assert s.atime.tzinfo is datetime.timezone.utc and s.mtime.tzinfo is datetime.timezone.utc
    assert s.mtime == datetime.datetime(2026, 1, 2, tzinfo=datetime.timezone.utc)
    assert convert_volume_entry_stat(raw()).target is None


def test_dual_method_picks_the_class_or_the_instance_function():
    class K:
        @classmethod
        def _c(cls, x):
            return ("class", cls.__name__, x)

        def _i(self, x):
            return ("instance", x)

        m = DualMethod(_c, _i)

    assert K.m(1) == ("class", "K", 1) and K().m(2) == ("instance", 2)


# ── volumes ────────────────────────────────────────────────────────────────


async def test_create_connect_get_info_list_destroy(backend):
    v = await AsyncVolume.create("data", domain="d.io", debug=True, proxy="http://p")
    assert (v.volume_id, v.name, v.token) == ("data", "data", "")
    assert (v._domain, v._debug, v._proxy) == ("d.io", True, "http://p")
    assert f"e2b-volumes/data/{MARKER}" in backend.volume.files
    again = await AsyncVolume.create("data")  # an existing volume is returned as it is
    assert again.volume_id == "data"
    await AsyncVolume.create("other")
    assert sorted(i.name for i in await AsyncVolume.list()) == ["data", "other"]
    assert all(isinstance(i, VolumeInfo) for i in await AsyncVolume.list())
    info = await AsyncVolume.get_info("data")
    assert info == VolumeAndToken(volume_id="data", name="data", token="", domain=None)
    c = await AsyncVolume.connect("data", domain="c.io", http_version="1.1")
    assert (c.volume_id, c.name, c.token, c._domain, c._http_version) == (
        "data", "data", "", "c.io", "1.1",
    )  # fmt: skip
    assert await AsyncVolume.destroy("data") is True
    assert await AsyncVolume.destroy("data") is False
    with pytest.raises(VolumeNotFoundException, match="Volume data not found"):
        await AsyncVolume.get_info("data")
    with pytest.raises(VolumeNotFoundException):
        await AsyncVolume.connect("data")
    assert [i.name for i in await AsyncVolume.list()] == ["other"]


async def test_volume_names_are_one_folder_name(backend):
    for bad in ("", "a/b", ".hidden"):
        with pytest.raises(InvalidArgumentException, match="one folder name"):
            await AsyncVolume.create(bad)


async def test_listing_before_any_volume_exists_is_empty(backend):
    assert await AsyncVolume.list() == []


async def test_the_constructor_keeps_what_e2b_keeps():
    v = AsyncVolume("id", "n", "tok", "dom", True, "http://p", http_version="1.1")
    assert (v.volume_id, v.name, v.token) == ("id", "n", "tok")
    assert (v._domain, v._debug, v._proxy, v._http_version) == ("dom", True, "http://p", "1.1")
    s = Volume("id", "n", "tok", "dom", True, "http://p", http_version="1.1")
    assert (s.volume_id, s.name, s.token) == ("id", "n", "tok")
    assert AsyncVolume("i", "n").token is None
    cfg = v._get_volume_config(request_timeout=3, headers={"X": "y"})
    assert (cfg.domain, cfg.debug, cfg.token, cfg.proxy, cfg.http_version) == (
        "dom", True, "tok", "http://p", "1.1",
    )  # fmt: skip
    assert cfg.request_timeout == 3 and cfg.headers["X"] == "y"
    assert v._get_volume_config(domain="o.io", debug=False, http_version="2").domain == "o.io"


async def test_class_level_options_resolve_through_the_client_factory(backend):
    class Bound(AsyncVolume):
        _bound_api_params = {"domain": "bound.example", "http_version": "1.1"}

    v = await Bound.create("b1")
    assert (v._domain, v._http_version) == ("bound.example", "1.1")
    v2 = await Bound.connect("b1", domain="call.example")
    assert v2._domain == "call.example" and isinstance(v2, Bound)


# ── write_file ─────────────────────────────────────────────────────────────


async def test_write_file_needs_its_parent_unless_forced(vol):
    with pytest.raises(VolumePathNotFoundException, match="Path /a/b.txt not found"):
        await vol.write_file("/a/b.txt", "x")
    with pytest.raises(VolumePathNotFoundException):
        await vol.write_file("/a/b.txt", "x", force=False)
    st = await vol.write_file("/a/b.txt", "x", force=True)
    assert st.path == "/a/b.txt" and st.type is VolumeFileType.FILE and st.size == 1
    await vol.make_dir("/d")
    await vol.write_file("/d/f", "y")  # an existing parent is enough
    await vol.write_file("/top", "z")  # the root always exists
    assert await vol.read_file("/a/b.txt") == "x"


async def test_write_file_takes_text_bytes_and_file_objects(vol):
    await vol.write_file("/s", "héllo")
    assert await vol.read_file("/s", format="bytes") == "héllo".encode()
    await vol.write_file("/b", b"\x00\xff")
    assert await vol.read_file("/b", format="bytes") == b"\x00\xff"
    await vol.write_file("/io", io.BytesIO(b"from io"))
    assert await vol.read_file("/io") == "from io"
    await vol.write_file("/text-io", io.StringIO("text io"))
    assert await vol.read_file("/text-io") == "text io"
    for bad in (bytearray(b"x"), 5, None, ["a"]):
        with pytest.raises(ValueError, match="Unsupported data type"):
            await vol.write_file("/bad", bad)  # type: ignore[arg-type]


async def test_write_file_overwrites_and_keeps_metadata_unless_given(vol):
    await vol.write_file("/f", "one", uid=5, gid=6, mode=0o600)
    st = await vol.write_file("/f", "three")
    assert (st.size, st.uid, st.gid, st.mode) == (5, 5, 6, 0o600)
    st = await vol.write_file("/f", "x", uid=9)
    assert (st.size, st.uid, st.gid, st.mode) == (1, 9, 6, 0o600)
    assert await vol.read_file("/f") == "x"


async def test_write_file_defaults_and_times(vol):
    st = await vol.write_file("/f", "x")
    assert (st.uid, st.gid, st.mode, st.name) == (0, 0, 0o644, "f")
    assert st.mtime.tzinfo is not None and st.target is None
    assert isinstance(st.atime, datetime.datetime) and st.ctime == st.mtime


async def test_write_file_refuses_directories_files_below_files_and_markers(vol):
    await vol.make_dir("/d")
    with pytest.raises(VolumeException, match="Path /d is a directory"):
        await vol.write_file("/d", "x")
    await vol.write_file("/f", "x")
    with pytest.raises(VolumeException, match="/f is a file, not a directory"):
        await vol.write_file("/f/inner", "x", force=True)
    for bad in ("/", "", f"/x/{MARKER}", "/x/.e2b-dir"):
        with pytest.raises(InvalidArgumentException, match="is not a file path"):
            await vol.write_file(bad, "x", force=True)
    with pytest.raises(InvalidArgumentException, match=r"'\.\.'"):
        await vol.write_file("/a/../b", "x")


async def test_files_land_in_the_volume_folder_of_cua_volume(vol, backend):
    await vol.write_file("/deep/er/f.bin", b"1", force=True)
    assert backend.volume.files["e2b-volumes/data/deep/er/f.bin"] == b"1"
    await vol.write_file("rel/path.txt", b"2", force=True)  # leading slash optional
    assert backend.volume.files["e2b-volumes/data/rel/path.txt"] == b"2"


# ── read_file ──────────────────────────────────────────────────────────────


async def test_read_file_formats(vol):
    payload = ("x" * 70_000).encode()
    await vol.write_file("/big", payload)
    assert await vol.read_file("/big") == payload.decode()
    assert await vol.read_file("/big", format="text") == payload.decode()
    assert await vol.read_file("/big", "bytes") == payload
    stream = await vol.read_file("/big", format="stream")
    chunks = [c async for c in stream]
    assert [len(c) for c in chunks] == [65536, 4464] and b"".join(chunks) == payload
    await vol.write_file("/empty", b"")
    assert [c async for c in await vol.read_file("/empty", format="stream")] == []
    assert await vol.read_file("/empty") == ""
    # stream_idle_timeout is accepted for every format
    assert b"".join([c async for c in await vol.read_file(
        "/big", "stream", stream_idle_timeout=0.5)]) == payload  # fmt: skip


async def test_read_file_errors(vol):
    with pytest.raises(VolumePathNotFoundException, match="Path /nope not found"):
        await vol.read_file("/nope")
    await vol.make_dir("/d")
    with pytest.raises(VolumeException, match="Path /d is a directory"):
        await vol.read_file("/d")
    # a stream reports a missing file when it is iterated, as a streamed HTTP read does
    stream = await vol.read_file("/nope", format="stream")
    with pytest.raises(VolumePathNotFoundException):
        await stream.__anext__()


# ── make_dir ───────────────────────────────────────────────────────────────


async def test_make_dir_needs_its_parent_unless_forced(vol):
    with pytest.raises(VolumePathNotFoundException, match="Path /a/b not found"):
        await vol.make_dir("/a/b")
    st = await vol.make_dir("/a/b", force=True)
    assert (st.path, st.type, st.size, st.mode) == ("/a/b", VolumeFileType.DIRECTORY, 0, 0o755)
    assert await vol.exists("/a/b")
    await vol.make_dir("/a/b/c")  # now its parent exists
    assert await vol.exists("/a/b/c")


async def test_make_dir_options_and_idempotence(vol):
    st = await vol.make_dir("/d", uid=3, gid=4, mode=0o700)
    assert (st.uid, st.gid, st.mode) == (3, 4, 0o700)
    again = await vol.make_dir("/d")  # an existing directory is returned as it is
    assert (again.uid, again.gid, again.mode) == (3, 4, 0o700)
    changed = await vol.make_dir("/d", mode=0o750)
    assert changed.mode == 0o750 and changed.uid == 3
    root = await vol.make_dir("/")
    assert root.path == "/" and root.type is VolumeFileType.DIRECTORY


async def test_make_dir_over_a_file_is_refused(vol):
    await vol.write_file("/f", "x")
    with pytest.raises(VolumeException, match="already exists and is a file"):
        await vol.make_dir("/f")


async def test_an_empty_directory_exists_and_lists_empty(vol, backend):
    await vol.make_dir("/empty")
    assert await vol.exists("/empty") and await vol.list("/empty") == []
    assert "e2b-volumes/data/empty/.e2b-dir" in backend.volume.files


# ── list / get_info / exists ───────────────────────────────────────────────


async def tree(vol):
    await vol.write_file("/a.txt", "a")
    await vol.write_file("/d/b.txt", "bb", force=True)
    await vol.write_file("/d/e/c.txt", "ccc", force=True)
    await vol.make_dir("/d/empty")


async def test_list_depth_default_one_and_recursive(vol):
    await tree(vol)
    assert sorted(names(await vol.list("/"))) == ["/a.txt", "/d"]
    assert sorted(names(await vol.list("/d"))) == ["/d/b.txt", "/d/e", "/d/empty"]
    assert sorted(names(await vol.list("/d", depth=1))) == ["/d/b.txt", "/d/e", "/d/empty"]
    assert sorted(names(await vol.list("/d", depth=2))) == [
        "/d/b.txt", "/d/e", "/d/e/c.txt", "/d/empty",
    ]  # fmt: skip
    deep = await vol.list("/", depth=10)
    assert sorted(names(deep)) == [
        "/a.txt", "/d", "/d/b.txt", "/d/e", "/d/e/c.txt", "/d/empty",
    ]  # fmt: skip
    assert await vol.list("/", depth=0) == []
    assert all(isinstance(e, VolumeEntryStat) for e in deep)
    kinds = {e.path: e.type for e in deep}
    assert kinds["/d"] is VolumeFileType.DIRECTORY and kinds["/a.txt"] is VolumeFileType.FILE
    sizes = {e.path: e.size for e in deep}
    assert sizes["/d/b.txt"] == 2 and sizes["/d/e/c.txt"] == 3 and sizes["/d"] == 0


async def test_list_hides_the_markers_and_shows_stored_metadata(vol):
    await vol.write_file("/f", "x", uid=7, gid=8, mode=0o640)
    await vol.make_dir("/d", mode=0o711)
    listing = {e.path: e for e in await vol.list("/")}
    assert set(listing) == {"/f", "/d"}  # not .e2b-volume.json, not .e2b-dir
    assert (listing["/f"].uid, listing["/f"].gid, listing["/f"].mode) == (7, 8, 0o640)
    assert listing["/d"].mode == 0o711


async def test_list_errors(vol):
    with pytest.raises(VolumePathNotFoundException, match="Path /nope not found"):
        await vol.list("/nope")
    await vol.write_file("/f", "x")
    with pytest.raises(VolumeException, match="Path /f is not a directory"):
        await vol.list("/f")


async def test_get_info_of_an_entry_and_of_the_volume_share_a_name(vol):
    await tree(vol)
    f = await vol.get_info("/d/b.txt")
    assert (f.name, f.path, f.size, f.type) == ("b.txt", "/d/b.txt", 2, VolumeFileType.FILE)
    d = await vol.get_info("/d")
    assert (d.name, d.type) == ("d", VolumeFileType.DIRECTORY)
    root = await vol.get_info("/")
    assert (root.path, root.type) == ("/", VolumeFileType.DIRECTORY)
    with pytest.raises(VolumePathNotFoundException, match="Path /x not found"):
        await vol.get_info("/x")
    info = await AsyncVolume.get_info("data")  # reached on the class: the volume itself
    assert isinstance(info, VolumeAndToken)
    assert [i.name for i in await AsyncVolume.list()] == ["data"]  # on the class: volumes


async def test_exists_for_files_directories_and_missing_paths(vol):
    await tree(vol)
    for path in ("/", "/a.txt", "/d", "/d/e", "/d/e/c.txt", "/d/empty", "d/b.txt"):
        assert await vol.exists(path), path
    for path in ("/nope", "/d/nope", "/a.txt/inner", "/d/e/c.txt/x"):
        assert not await vol.exists(path), path


# ── update_metadata ────────────────────────────────────────────────────────


async def test_update_metadata_changes_only_what_is_given(vol):
    await vol.write_file("/f", "x", uid=1, gid=2, mode=0o600)
    st = await vol.update_metadata("/f", gid=20)
    assert (st.uid, st.gid, st.mode) == (1, 20, 0o600)
    st = await vol.update_metadata("/f", uid=10, mode=0o644)
    assert (st.uid, st.gid, st.mode) == (10, 20, 0o644)
    assert (await vol.update_metadata("/f")).uid == 10  # nothing given: the stat
    await vol.make_dir("/d")
    d = await vol.update_metadata("/d", uid=3, mode=0o700)
    assert (d.type, d.uid, d.mode) == (VolumeFileType.DIRECTORY, 3, 0o700)
    assert (await vol.get_info("/d")).uid == 3
    root = await vol.update_metadata("/", uid=1)
    assert root.uid == 1
    with pytest.raises(VolumePathNotFoundException, match="Path /nope not found"):
        await vol.update_metadata("/nope", uid=1)


# ── remove ─────────────────────────────────────────────────────────────────


async def test_remove_a_file_a_directory_tree_and_the_missing(vol, backend):
    await tree(vol)
    await vol.update_metadata("/d/e/c.txt", uid=4)
    await vol.remove("/a.txt")
    assert not await vol.exists("/a.txt")
    await vol.remove("/d")  # recursive
    assert not await vol.exists("/d") and not await vol.exists("/d/e/c.txt")
    assert await vol.list("/") == []
    assert [k for k in backend.volume.files if "/d/" in k] == []
    meta = json.loads(backend.volume.files[f"e2b-volumes/data/{MARKER}"])
    assert "d/e/c.txt" not in meta["meta"]  # its metadata went with it
    with pytest.raises(VolumePathNotFoundException, match="Path /d not found"):
        await vol.remove("/d")
    with pytest.raises(VolumeException, match="root of a volume"):
        await vol.remove("/")
    await vol.write_file("/d/new", "n", force=True)  # a removed name starts fresh
    assert (await vol.get_info("/d/new")).uid == 0


async def test_destroy_removes_every_file_and_metadata(vol, backend):
    await tree(vol)
    assert await AsyncVolume.destroy("data") is True
    assert not [k for k in backend.volume.files if k.startswith("e2b-volumes/data")]


# ── the blocking Volume ────────────────────────────────────────────────────


def test_volume_sync_runs_every_call(backend):
    v = Volume.create("sync", domain="s.io")
    assert (v.volume_id, v.name, v.token, v._domain) == ("sync", "sync", "", "s.io")
    with pytest.raises(VolumePathNotFoundException):
        v.write_file("/d/x", "1")
    st = v.write_file("/d/x", "1", uid=2, gid=3, mode=0o600, force=True)
    assert (st.uid, st.gid, st.mode) == (2, 3, 0o600)
    assert v.make_dir("/e", mode=0o700, force=True).mode == 0o700
    assert v.read_file("/d/x") == "1" and v.read_file("/d/x", "bytes") == b"1"
    assert b"".join(v.read_file("/d/x", format="stream", stream_idle_timeout=1)) == b"1"
    assert sorted(names(v.list("/", depth=2))) == ["/d", "/d/x", "/e"]
    assert v.get_info("/d/x").size == 1
    assert v.exists("/d/x") and not v.exists("/zz")
    assert v.update_metadata("/d/x", uid=9).uid == 9
    v.remove("/d")
    assert not v.exists("/d/x")
    with pytest.raises(VolumePathNotFoundException):
        v.remove("/d")
    with pytest.raises(VolumePathNotFoundException):
        v.read_file("/d/x")
    # class level: the same names address volumes
    assert isinstance(Volume.get_info("sync"), VolumeAndToken)
    assert [i.name for i in Volume.list()] == ["sync"]
    c = Volume.connect("sync", debug=True)
    assert isinstance(c, Volume) and c._debug is True and c.name == "sync"
    assert Volume.destroy("sync") is True and Volume.destroy("sync") is False
    with pytest.raises(VolumeNotFoundException):
        Volume.connect("sync")


def test_a_sync_stream_reports_a_missing_file_when_iterated(backend):
    v = Volume.create("sync2")
    stream = v.read_file("/nope", format="stream")
    with pytest.raises(VolumePathNotFoundException):
        next(stream)


def test_volume_sync_accepts_file_objects_and_rejects_other_data(backend):
    v = Volume.create("sync3")
    v.write_file("/f", io.BytesIO(b"bytes io"))
    assert v.read_file("/f") == "bytes io"
    with pytest.raises(ValueError, match="Unsupported data type"):
        v.write_file("/g", 3)  # type: ignore[arg-type]


def test_the_volume_marker_file_is_not_listed_as_a_volume_entry(backend):
    v = Volume.create("sync4")
    assert v.list("/") == []
    assert os.path.basename(MARKER) == ".e2b-volume.json"

"""Runtime behaviours pinned against e2b 2.58 (in-memory backend)."""

from __future__ import annotations

import asyncio
import io

import pytest
from e2b import (
    AsyncSandbox,
    InvalidArgumentException,
    NotFoundException,
    SandboxException,
)
from e2b._backend import set_backend
from fake_backend import FakeBackend


@pytest.fixture
def backend():
    b = FakeBackend()
    set_backend(b)
    yield b
    set_backend(None)


async def _sb():
    return await AsyncSandbox.create(template="w")


async def test_stream_input_delivers_in_order_and_closes(backend):
    sb = await _sb()
    h = await sb.commands.run("forever", background=True, timeout=0, stdin=True)
    async with sb.commands.stream_input(h.pid) as inp:
        await inp.send("a")
        await inp.send(b"b")
        await inp.send("c")
    proc = backend.machines[sb.sandbox_id].procs[h.pid]
    assert proc.stdin == [b"a", b"b", b"c"]
    with pytest.raises(
        SandboxException, match="Cannot send input after the input handle is closed"
    ):
        await inp.send("d")
    await h.kill()


async def test_pty_stream_input_goes_to_the_terminal(backend):
    from e2b import PtySize

    sb = await _sb()
    h = await sb.pty.create(PtySize(rows=24, cols=80), lambda d: None)
    inp = sb.pty.stream_input(h.pid)
    await inp.send("ls\n")
    await inp.close()
    assert backend.machines[sb.sandbox_id].procs[h.pid].pty_in == [b"ls\n"]


async def test_stream_input_unknown_pid_surfaces_not_found(backend):
    sb = await _sb()
    inp = sb.commands.stream_input(424242)
    await asyncio.sleep(0.05)
    with pytest.raises(NotFoundException):
        await inp.close()


async def test_unknown_pid_is_not_found_not_sandbox_not_found(backend):
    sb = await _sb()
    with pytest.raises(NotFoundException) as info:
        await sb.commands.connect(999999)
    assert type(info.value) is NotFoundException
    with pytest.raises(NotFoundException):
        await sb.commands.send_stdin(999999, "x")


async def test_list_depth_must_be_at_least_one(backend):
    sb = await _sb()
    for bad in (0, -1):
        with pytest.raises(InvalidArgumentException, match="depth should be at least 1"):
            await sb.files.list("/tmp", depth=bad)


async def test_write_rejects_unsupported_data_like_e2b(backend):
    sb = await _sb()
    with pytest.raises(InvalidArgumentException, match="Unsupported data type"):
        await sb.files.write("/tmp/x", bytearray(b"x"))
    with pytest.raises(InvalidArgumentException, match="Unsupported data type for file /tmp/x"):
        await sb.files.write("/tmp/x", bytearray(b"x"), use_octet_stream=False)


async def test_write_streams_file_objects_and_encodes_text_io(backend):
    sb = await _sb()
    big = b"\x00\xff" * (1024 * 1024 + 17)
    await sb.files.write("/tmp/bin", io.BytesIO(big))
    assert backend.machines[sb.sandbox_id].files["/tmp/bin"] == big
    await sb.files.write("/tmp/txt", io.StringIO("héllo"))
    assert backend.machines[sb.sandbox_id].files["/tmp/txt"] == "héllo".encode()


async def test_write_files_empty_list_and_dict_entries(backend):
    sb = await _sb()
    assert await sb.files.write_files([]) == []
    infos = await sb.files.write_files(
        [{"path": "/tmp/a", "data": "1"}, {"path": "/tmp/b", "data": b"2"}]
    )
    assert [i.path for i in infos] == ["/tmp/a", "/tmp/b"]


async def test_metadata_is_validated_before_anything_is_written(backend):
    sb = await _sb()
    with pytest.raises(InvalidArgumentException, match="Invalid metadata key"):
        await sb.files.write("/tmp/m", "x", metadata={"bad key": "v"})
    with pytest.raises(InvalidArgumentException, match="printable US-ASCII"):
        await sb.files.write("/tmp/m", "x", metadata={"k": "é"})
    assert "/tmp/m" not in backend.machines[sb.sandbox_id].files


async def test_watch_on_exit_fires_once_with_none_on_stop(backend):
    sb = await _sb()
    exits = []
    h = await sb.files.watch_dir("/w", lambda e: None, on_exit=exits.append)
    await h.stop()
    await h.stop()
    assert exits == [None]


async def test_watch_timeout_ends_with_a_timeout_exception(backend):
    from e2b import TimeoutException

    sb = await _sb()
    exits = []
    # as in e2b, the timeout cancels the task that opened the watch
    opener = asyncio.ensure_future(
        sb.files.watch_dir("/w", lambda e: None, on_exit=exits.append, timeout=0.3)
    )
    await opener
    await asyncio.sleep(0.8)
    assert len(exits) == 1 and isinstance(exits[0], TimeoutException)


async def test_watch_include_entry(backend):
    sb = await _sb()
    got = []
    h = await sb.files.watch_dir("/w", got.append, include_entry=True)
    await sb.files.write("/w/f", "1")
    await asyncio.sleep(0.5)
    await h.stop()
    assert got and got[0].entry is not None and got[0].entry.name == "f"
    got2 = []
    h = await sb.files.watch_dir("/w", got2.append)
    await sb.files.write("/w/g", "1")
    await asyncio.sleep(0.5)
    await h.stop()
    assert got2 and got2[0].entry is None


# -- URLs, network updates, class-level calls, pagination -----------------------


async def test_download_and_upload_urls_are_signed_by_spacesd(backend):
    sb = await _sb()
    d = sb.download_url("/tmp/a.txt")
    assert d.startswith("http://10.0.0.5:3211/files?") and "method=GET" in d and "exp=86400" in d
    u = sb.upload_url("/tmp/a.txt", use_signature_expiration=60)
    assert "method=PUT" in u and "exp=60" in u
    for bad in (0, -5, 86401):
        with pytest.raises(InvalidArgumentException, match="between 1 and 86400"):
            sb.download_url("/tmp/a", use_signature_expiration=bad)
    with pytest.raises(InvalidArgumentException, match="default user"):
        sb.download_url("/tmp/a", user="root")
    assert sb.envd_api_url == "http://10.0.0.5:3211" and sb.sandbox_domain == "10.0.0.5"


async def test_update_network_replaces_the_rules(backend):
    sb = await _sb()
    m = backend.machines[sb.sandbox_id]
    await sb.update_network({"deny_out": ["10.0.0.0/8"], "allow_out": ["10.1.0.0/16"]})
    assert m.egress == {"allow": ["10.1.0.0/16"], "deny": ["10.0.0.0/8"]}
    await sb.update_network({"allow_internet_access": False})
    assert m.egress == {"allow": [], "deny": ["0.0.0.0/0"]}
    assert (await sb.get_info()).allow_internet_access is False
    await sb.update_network({})  # omitted fields are cleared, as in e2b
    assert m.egress == {"allow": [], "deny": []}
    assert (await sb.get_info()).allow_internet_access is True
    with pytest.raises(InvalidArgumentException, match="TLS-intercepting"):
        await sb.update_network({"egress_proxy": {"address": "p:1"}})
    await AsyncSandbox.update_network(sb.sandbox_id, {"deny_out": ["1.1.1.1"]})
    assert m.egress["deny"] == ["1.1.1.1"]


async def test_class_level_calls_take_a_sandbox_id(backend):
    sb = await AsyncSandbox.create(template="w", metadata={"k": "v"})
    info = await AsyncSandbox.get_info(sb.sandbox_id)
    assert info.sandbox_id == sb.sandbox_id and info.metadata == {"k": "v"}
    assert str(info.state) == "running"
    [m] = await AsyncSandbox.get_metrics(sb.sandbox_id)
    assert m.cpu_count == 2
    assert await AsyncSandbox.pause(sb.sandbox_id) is True
    assert (await AsyncSandbox.get_info(sb.sandbox_id)).state == "paused"
    snap = await AsyncSandbox.create_snapshot(sb.sandbox_id, "s1")
    assert snap.snapshot_id == "s1"
    from e2b import SandboxNotFoundException

    with pytest.raises(SandboxNotFoundException):
        await AsyncSandbox.get_info("e2b-nope")


async def test_get_info_of_a_paused_sandbox_does_not_need_its_guest(backend):
    sb = await _sb()
    await sb.get_info()  # measures and records the size
    await sb.pause()
    info = await sb.get_info()
    assert info.state == "paused" and info.cpu_count == 2 and info.memory_mb == 3906


async def test_listing_pages_with_a_cursor_and_loses_nothing(backend):
    made = [await AsyncSandbox.create(template="w", metadata={"n": str(i)}) for i in range(5)]
    pager = AsyncSandbox.list(limit=2)
    seen, pages = [], 0
    while pager.has_next:
        items = await pager.next_items()
        pages += 1
        assert len(items) <= 2
        seen += [i.sandbox_id for i in items]
    assert pages == 3 and sorted(seen) == sorted(m.sandbox_id for m in made)
    second = AsyncSandbox.list(limit=2)
    await second.next_items()
    resumed = AsyncSandbox.list(limit=2, next_token=second.next_token)
    assert [i.sandbox_id for i in await resumed.next_items()] == seen[2:4]
    with pytest.raises(SandboxException, match="^400: Invalid next token$"):
        await AsyncSandbox.list(next_token="bogus").next_items()


async def test_reconnect_keeps_what_was_recorded(backend):
    sb = await AsyncSandbox.create(template="w", metadata={"run": "9"}, timeout=500)
    again = await AsyncSandbox.connect(sb.sandbox_id)
    info = await again.get_info()
    assert info.metadata == {"run": "9"} and info.template_id == "w"
    assert 480 < (info.end_at - info.started_at).total_seconds() < 520


async def test_connect_timeout_ends_the_stream_but_not_the_command(backend):
    from e2b import TimeoutException

    sb = await _sb()
    h = await sb.commands.run("forever", background=True, timeout=0)
    h2 = await sb.commands.connect(h.pid, timeout=0.3)
    with pytest.raises(TimeoutException):
        await h2.wait()
    assert await sb.commands.kill(h.pid) is True  # still running

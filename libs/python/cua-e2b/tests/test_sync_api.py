"""The synchronous API over the in-memory backend: the same semantics as the
async one, with e2b's sync shapes (iterating handles, polled watches, stream
readers)."""

from __future__ import annotations

import asyncio
import inspect
import io
import time

import pytest
from e2b import (
    CommandExitException,
    InvalidArgumentException,
    NotFoundException,
    PtySize,
    Sandbox,
    SandboxException,
)
from e2b._backend import set_backend
from e2b.sandbox.filesystem.filesystem import FileStreamReader
from e2b.sandbox_sync.commands.command_handle import CommandHandle
from fake_backend import FakeBackend


@pytest.fixture
def backend():
    b = FakeBackend()
    b.permissive = True
    set_backend(b)
    yield b
    set_backend(None)


def test_foreground_run_and_failure(backend):
    with Sandbox.create(template="w") as sb:
        r = sb.commands.run("echo hi")
        assert (r.stdout, r.exit_code) == ("hi\n", 0)
        with pytest.raises(CommandExitException) as e:
            sb.commands.run("fail 4")
        assert e.value.exit_code == 4


def test_foreground_callbacks_receive_output(backend):
    sb = Sandbox.create(template="w")
    seen = []
    sb.commands.run(
        "chunks",
        on_stdout=lambda t: seen.append(("o", t)),
        on_stderr=lambda t: seen.append(("e", t)),
    )
    assert "".join(t for k, t in seen if k == "o") == "hello é\n"
    assert ("e", "warn") in seen


def test_background_handle_iterates_output_then_ends(backend):
    sb = Sandbox.create(template="w")
    h = sb.commands.run("chunks", background=True)
    assert isinstance(h, CommandHandle) and isinstance(h.pid, int)
    out, err = "", ""
    for stdout, stderr, pty in h:
        assert pty is None
        out += stdout or ""
        err += stderr or ""
    assert (out, err) == ("hello é\n", "warn")
    r = h.wait()
    assert r.exit_code == 0 and r.stdout == out
    assert not any(hasattr(h, a) for a in ("stdout", "stderr", "error", "exit_code"))
    assert list(h) == []  # a second iteration ends at once


def test_wait_streams_to_callbacks_and_raises_on_nonzero(backend):
    sb = Sandbox.create(template="w")
    seen = []
    h = sb.commands.run("fail 3", background=True)
    with pytest.raises(CommandExitException):
        h.wait(on_stdout=seen.append)
    assert seen == ["out"]


def test_kill_disconnect_and_unknown_pid(backend):
    sb = Sandbox.create(template="w")
    h = sb.commands.run("forever", background=True, timeout=0)
    h.disconnect()
    assert sb.commands.kill(h.pid) is True
    with pytest.raises(NotFoundException):
        sb.commands.connect(999999)
    assert sb.commands.kill(999999) is False


def test_stdin_and_stream_input(backend):
    sb = Sandbox.create(template="w")
    h = sb.commands.run("forever", background=True, stdin=True, timeout=0)
    h.send_stdin("a")
    sb.commands.stream_input(h.pid, ["b", b"c"])
    assert backend.machines[sb.sandbox_id].procs[h.pid].stdin == [b"a", b"b", b"c"]
    with pytest.raises(InvalidArgumentException, match="iterable of str or bytes chunks"):
        sb.commands.stream_input(h.pid, "abc")
    h.kill()


def test_pty_output_through_the_handle(backend):
    sb = Sandbox.create(template="w")
    h = sb.pty.create(PtySize(rows=24, cols=80))
    sb.pty.send_stdin(h.pid, b"ls\n")
    stdout, stderr, data = next(iter(h))
    assert data == b"ls\n" and stdout is None and stderr is None
    sb.pty.resize(h.pid, PtySize(rows=40, cols=120))
    assert sb.pty.kill(h.pid) is True


def test_files_round_trip_stream_and_info(backend):
    sb = Sandbox.create(template="w")
    data = bytes(range(256)) * 5000
    info = sb.files.write("/tmp/a", data)
    assert info.path == "/tmp/a"
    assert bytes(sb.files.read("/tmp/a", format="bytes")) == data
    with sb.files.read("/tmp/a", format="stream") as stream:
        assert isinstance(stream, FileStreamReader)
        assert b"".join(stream) == data
    sb.files.write("/tmp/b", io.BytesIO(b"x"))
    assert sb.files.exists("/tmp/b") and sb.files.get_info("/tmp/b").size == 1
    assert sb.files.write_files([]) == []
    with pytest.raises(InvalidArgumentException, match="depth should be at least 1"):
        sb.files.list("/tmp", depth=0)
    sb.files.remove("/tmp/b")
    assert not sb.files.exists("/tmp/b")


def test_watch_is_polled_and_stop_is_final(backend):
    sb = Sandbox.create(template="w")
    w = sb.files.watch_dir("/w", include_entry=True)
    sb.files.write("/w/f", "1")
    events = w.get_new_events()
    assert [e.name for e in events] == ["f"] and events[0].entry is not None
    assert w.get_new_events() == []
    w.stop()
    with pytest.raises(SandboxException, match="The watcher is already stopped"):
        w.get_new_events()


def test_pagination_and_class_level_calls(backend):
    made = [Sandbox.create(template="w", metadata={"i": str(i)}) for i in range(3)]
    pager = Sandbox.list(limit=2)
    ids = []
    while pager.has_next:
        ids += [i.sandbox_id for i in pager.next_items()]
    assert sorted(ids) == sorted(m.sandbox_id for m in made)
    assert Sandbox.get_info(made[0].sandbox_id).metadata == {"i": "0"}
    assert Sandbox.set_timeout(made[0].sandbox_id, 99) is None
    assert Sandbox.kill(made[0].sandbox_id) is True
    assert Sandbox.kill(made[0].sandbox_id) is False


def test_sync_calls_work_from_inside_a_running_event_loop(backend):
    async def inside():
        sb = Sandbox.create(template="w")  # blocks this loop's thread only
        return sb.commands.run("echo ok").stdout

    assert asyncio.run(inside()) == "ok\n"


def test_signatures_are_the_async_ones(backend):
    sb = Sandbox.create(template="w")
    for name in ("write", "list", "get_info", "exists", "remove", "rename", "make_dir"):
        assert "request_timeout" in inspect.signature(getattr(sb.files, name)).parameters
    assert "background" in inspect.signature(sb.commands.run).parameters
    assert sb.files.write.__wrapped__.__name__ == "write"


def test_urls_and_properties(backend):
    sb = Sandbox.create(template="w")
    assert sb.download_url("/tmp/x").startswith("http://10.0.0.5:3211/files?")
    assert sb.envd_api_url == sb.envd_direct_url and sb.connection_config.domain
    assert Sandbox.default_template == "base" and Sandbox.default_sandbox_timeout == 300
    t0 = time.time()
    sb.update_network({"deny_out": ["10.0.0.0/8"]})
    assert time.time() - t0 < 5

"""e2b semantics, over an in-memory backend (no VMs, no host processes)."""

from __future__ import annotations

import asyncio
import json

import e2b
import pytest
from e2b import (
    AsyncCommandHandle,
    AsyncSandbox,
    CommandExitException,
    CommandResult,
    FileNotFoundException,
    Sandbox,
    SandboxException,
    SandboxNotFoundException,
    TemplateException,
    TimeoutException,
)
from e2b._backend import resolve_template, set_backend
from e2b._errors import map_error
from fake_backend import FakeBackend, NotFound, Timeout, Transport


@pytest.fixture
def backend(monkeypatch):
    monkeypatch.delenv("CUA_E2B_TEMPLATES", raising=False)
    b = FakeBackend()
    set_backend(b)
    yield b
    set_backend(None)


async def _sandbox(backend, **kw) -> AsyncSandbox:
    short = kw.get("timeout")
    if isinstance(short, float) and not short.is_integer():
        # e2b's API takes whole seconds; tests that need a lifetime of a few
        # milliseconds set it on the handle
        kw.pop("timeout")
        sb = await AsyncSandbox.create(template="world-1", **kw)
        await sb._set_lifetime(short)
        return sb
    return await AsyncSandbox.create(template="world-1", **kw)


# ── commands.run: foreground ───────────────────────────────────────────────


async def test_foreground_run_returns_command_result(backend):
    sb = await _sandbox(backend)
    r = await sb.commands.run("echo hi", user="root", timeout=30)
    assert isinstance(r, CommandResult)
    assert (r.stdout, r.stderr, r.exit_code, r.error) == ("hi\n", "", 0, None)
    spawned = backend.machines[sb.sandbox_id].spawned[-1]
    # e2b runs the line with a login bash.
    assert spawned["argv"] == ["/bin/bash", "-l", "-c", "echo hi"]
    assert spawned["user"] == "root"
    assert spawned["timeout_ms"] == 30_000


async def test_nonzero_exit_raises_command_exit_exception(backend):
    sb = await _sandbox(backend)
    with pytest.raises(CommandExitException) as info:
        await sb.commands.run("fail 3")
    exc = info.value
    assert (exc.exit_code, exc.stdout, exc.stderr) == (3, "out", "err")
    # It is both a SandboxException and a CommandResult, like e2b's.
    assert isinstance(exc, SandboxException) and isinstance(exc, CommandResult)
    assert str(exc) == "Command exited with code 3 and error:\nerr"


async def test_exit_exception_is_a_result_for_callers(backend):
    # A typical caller: `except CommandExitException as exc: result = exc`.
    sb = await _sandbox(backend)
    try:
        result = await sb.commands.run("fail 1")
    except CommandExitException as exc:
        result = exc
    assert (result.exit_code, result.stdout, result.stderr) == (1, "out", "err")


async def test_default_user_and_envs(backend):
    sb = await _sandbox(backend, envs={"A": "1", "B": "1"})
    backend.machines[sb.sandbox_id].dirs.add("/w")
    await sb.commands.run("echo x", envs={"B": "2"}, user="user", cwd="/w")
    s = backend.machines[sb.sandbox_id].spawned[-1]
    assert s["user"] is None  # e2b's "user" is the Space's default user
    assert s["env"] == {"A": "1", "B": "2"}
    assert s["cwd"] == "/w"


async def test_timeout_zero_is_unlimited_and_default_is_60s(backend):
    sb = await _sandbox(backend)
    await sb.commands.run("echo x", timeout=0)
    await sb.commands.run("echo x")
    spawned = backend.machines[sb.sandbox_id].spawned
    assert [s["timeout_ms"] for s in spawned] == [None, 60_000]


async def test_command_past_its_timeout_raises_timeout(backend):
    sb = await _sandbox(backend)
    with pytest.raises(TimeoutException):
        await sb.commands.run("forever", timeout=0.05)


async def test_request_timeout_bounds_the_start(backend):
    sb = await _sandbox(backend)
    with pytest.raises(TimeoutException):
        await sb.commands.run("hang-start", request_timeout=0.05)


async def test_utf8_split_across_chunks_and_callbacks(backend):
    sb = await _sandbox(backend)
    out, err = [], []
    r = await sb.commands.run("chunks", on_stdout=out.append, on_stderr=err.append)
    assert r.stdout == "hello é\n"
    assert "".join(out) == "hello é\n" and err == ["warn"]


async def test_async_callbacks_are_awaited(backend):
    sb = await _sandbox(backend)
    seen = []

    async def cb(text):
        await asyncio.sleep(0)
        seen.append(text)

    await sb.commands.run("echo hi", on_stdout=cb)
    assert seen == ["hi\n"]


async def test_unknown_command_is_exit_127(backend):
    sb = await _sandbox(backend)
    with pytest.raises(CommandExitException) as info:
        await sb.commands.run("nope")
    assert info.value.exit_code == 127


# ── commands.run: background handles ───────────────────────────────────────


async def test_background_returns_handle_then_wait(backend):
    sb = await _sandbox(backend)
    h = await sb.commands.run("echo bg", background=True)
    assert isinstance(h, AsyncCommandHandle)
    assert isinstance(h.pid, int)
    r = await h.wait()
    assert r.stdout == "bg\n" and h.exit_code == 0 and h.stdout == "bg\n"


async def test_background_wait_raises_on_nonzero(backend):
    sb = await _sandbox(backend)
    h = await sb.commands.run("fail 2", background=True)
    with pytest.raises(CommandExitException) as info:
        await h.wait()
    assert info.value.exit_code == 2


async def test_collector_pattern_reply_then_disconnect(backend):
    """An RL tool collector: background, timeout=0, on_stdout parses the
    first line; then disconnect, which must not kill the process."""
    sb = await _sandbox(backend)
    reply = asyncio.get_running_loop().create_future()
    chunks = []

    def receive(text):
        if reply.done():
            return
        line, sep, _ = text.partition("\n")
        chunks.append(line)
        if sep:
            reply.set_result(json.loads("".join(chunks)))

    h = await sb.commands.run("reply", background=True, timeout=0, user="root", on_stdout=receive)
    waiter = asyncio.create_task(h.wait())
    await asyncio.wait((reply, waiter), return_when=asyncio.FIRST_COMPLETED)
    assert reply.result() == {"ok": True}
    assert h.exit_code is None  # still running
    await h.disconnect()
    await asyncio.gather(waiter, return_exceptions=True)
    proc = backend.machines[sb.sandbox_id].procs[h.pid]
    assert proc.detached and not proc.killed


async def test_handle_kill_then_wait(backend):
    sb = await _sandbox(backend)
    h = await sb.commands.run("forever", background=True, timeout=0)
    assert await h.kill() is True
    with pytest.raises(CommandExitException) as info:
        await h.wait()
    assert info.value.exit_code == -1


async def test_commands_kill_by_pid_and_unknown_pid(backend):
    sb = await _sandbox(backend)
    h = await sb.commands.run("forever", background=True, timeout=0)
    assert [p.pid for p in await sb.commands.list()] == [h.pid]
    assert await sb.commands.kill(h.pid) is True
    assert await sb.commands.kill(99999) is False


async def test_send_stdin(backend):
    sb = await _sandbox(backend)
    h = await sb.commands.run("forever", background=True, timeout=0, stdin=True)
    await h.send_stdin("abc")
    await sb.commands.send_stdin(h.pid, b"def")
    assert backend.machines[sb.sandbox_id].procs[h.pid].stdin == [b"abc", b"def"]
    assert backend.machines[sb.sandbox_id].spawned[-1]["stdin"] is True


# ── files ──────────────────────────────────────────────────────────────────


async def test_file_round_trip(backend):
    sb = await _sandbox(backend)
    info = await sb.files.write("/tmp/a.txt", "héllo", user="root", request_timeout=600)
    assert (info.name, info.path, info.type) == ("a.txt", "/tmp/a.txt", e2b.FileType.FILE)
    assert await sb.files.read("/tmp/a.txt") == "héllo"
    assert await sb.files.read("/tmp/a.txt", format="bytes") == bytearray("héllo".encode())
    assert await sb.files.exists("/tmp/a.txt")
    await sb.files.remove("/tmp/a.txt", user="root", request_timeout=120)
    assert not await sb.files.exists("/tmp/a.txt")


async def test_write_accepts_file_objects(backend):
    import io

    sb = await _sandbox(backend)
    await sb.files.write("/tmp/b", io.BytesIO(b"\x00\x01"))
    assert backend.machines[sb.sandbox_id].files["/tmp/b"] == b"\x00\x01"


async def test_relative_paths_resolve_against_home(backend):
    sb = await _sandbox(backend)
    await sb.files.write("x.txt", "1")
    await sb.files.write("y.txt", "2", user="root")
    files = backend.machines[sb.sandbox_id].files
    assert set(files) == {"/home/cua/x.txt", "/root/y.txt"}


async def test_read_stream(backend):
    sb = await _sandbox(backend)
    payload = bytes(range(256)) * 4096  # 1 MiB
    await sb.files.write("/tmp/big", payload)
    got = bytearray()
    async with await sb.files.read(
        "/tmp/big", format="stream", user="root", request_timeout=60
    ) as stream:
        async for chunk in stream:
            got += chunk
    assert bytes(got) == payload


async def test_missing_file_raises_file_not_found(backend):
    sb = await _sandbox(backend)
    with pytest.raises(FileNotFoundException):
        await sb.files.read("/nope")
    with pytest.raises(FileNotFoundException):
        await sb.files.get_info("/nope")


# ── sandbox lifecycle ──────────────────────────────────────────────────────


async def test_get_host_is_sync_and_cached(backend):
    sb = await _sandbox(backend)
    host = sb.get_host(8080)
    assert host == "127.0.0.1:48080"
    assert sb.get_host(8080) is host
    assert "://" not in host


async def test_kill_deletes_once(backend):
    sb = await _sandbox(backend)
    assert await sb.kill(request_timeout=120) is True
    assert backend.machines[sb.sandbox_id].deleted
    assert await sb.kill() is False


async def test_kill_by_id_classmethod(backend):
    sb = await _sandbox(backend)
    assert await AsyncSandbox.kill(sb.sandbox_id) is True
    assert await AsyncSandbox.kill("missing") is False


async def test_lifetime_timeout_kills_and_set_timeout_extends(backend):
    sb = await _sandbox(backend, timeout=0.05)
    other = await _sandbox(backend, timeout=0.05)
    await other.set_timeout(5)
    await asyncio.sleep(0.15)
    assert backend.machines[sb.sandbox_id].deleted
    assert not backend.machines[other.sandbox_id].deleted
    await other.kill()


async def test_create_passes_template_and_maps_errors(backend):
    sb = await _sandbox(backend)
    assert backend.machines[sb.sandbox_id].template == "world-1"
    backend.fail_create = NotFound("image not found")
    # e2b: a plain SandboxException carrying the API's text
    with pytest.raises(SandboxException) as ei:
        await AsyncSandbox.create(template="nope")
    assert type(ei.value) is SandboxException
    assert str(ei.value) == "404: template 'nope' not found"
    with pytest.raises(
        SandboxException, match="^400: Invalid template reference: invalid template ID: No Way!$"
    ):
        await AsyncSandbox.create(template="No Way!")


async def test_create_snapshot_and_pause(backend):
    sb = await _sandbox(backend)
    snap = await sb.create_snapshot("baked")
    assert snap.snapshot_id == "baked" and snap.names == ["baked"]
    assert await sb.pause() is True
    await sb.connect()
    assert not backend.machines[sb.sandbox_id].suspended


async def test_unsupported_create_options_refuse(backend):
    with pytest.raises(SandboxException):
        await AsyncSandbox.create(template="x", mcp={"github": {}})


async def test_async_context_manager_kills(backend):
    async with await _sandbox(backend) as sb:
        pass
    assert backend.machines[sb.sandbox_id].deleted


# ── templates and errors ───────────────────────────────────────────────────


def test_template_mapping(monkeypatch, tmp_path):
    monkeypatch.setenv("CUA_E2B_TEMPLATES", json.dumps({"snap_abc": "world-ckpt"}))
    assert resolve_template("snap_abc") == "world-ckpt"
    assert resolve_template("other") == "other"
    path = tmp_path / "m.json"
    path.write_text(json.dumps({"snap_abc": "from-file"}))
    monkeypatch.setenv("CUA_E2B_TEMPLATES", str(path))
    assert resolve_template("snap_abc") == "from-file"
    monkeypatch.delenv("CUA_E2B_TEMPLATES")
    monkeypatch.setenv("CUA_E2B_IMAGE", "ghcr.io/x/linux:1")
    assert resolve_template(None) == "ghcr.io/x/linux:1"
    assert resolve_template("base") == "ghcr.io/x/linux:1"


def test_error_mapping():
    assert isinstance(map_error(NotFound("x")), SandboxNotFoundException)
    assert isinstance(map_error(NotFound("x"), file=True), FileNotFoundException)
    assert isinstance(map_error(Timeout("x")), TimeoutException)
    t = map_error(Transport("reset"))
    assert type(t) is SandboxException  # transient for retry policies
    plain = ValueError("x")
    assert map_error(plain) is plain


def test_exception_hierarchy_matches_e2b():
    assert issubclass(e2b.FileNotFoundException, e2b.NotFoundException)
    assert issubclass(e2b.SandboxNotFoundException, e2b.NotFoundException)
    assert issubclass(e2b.NotFoundException, e2b.SandboxException)
    assert issubclass(e2b.TemplateException, e2b.SandboxException)
    assert not issubclass(e2b.AuthenticationException, e2b.SandboxException)
    assert issubclass(e2b.TimeoutException, e2b.SandboxException)


# ── synchronous Sandbox ────────────────────────────────────────────────────


def test_sync_sandbox(backend):
    sb = Sandbox.create(template="world-1")
    assert sb.commands.run("echo s").stdout == "s\n"
    with pytest.raises(CommandExitException):
        sb.commands.run("fail 4")
    h = sb.commands.run("echo bg", background=True)
    assert h.wait().stdout == "bg\n"
    sb.files.write("/tmp/s", b"abc")
    assert sb.files.read("/tmp/s") == "abc"
    assert b"".join(sb.files.read("/tmp/s", format="stream")) == b"abc"
    assert sb.get_host(9000) == "127.0.0.1:49000"
    assert sb.kill() is True


# ── behaviour observed against real e2b 2.42 sandboxes (conformance run) ──


async def test_error_strings_match_e2b(backend):
    sb = await _sandbox(backend)
    with pytest.raises(CommandExitException) as info:
        await sb.commands.run("fail 3")
    assert info.value.error == "exit status 3"
    h = await sb.commands.run("forever", background=True, timeout=0)
    await h.kill()
    with pytest.raises(CommandExitException) as info:
        await h.wait()
    assert (info.value.exit_code, info.value.error) == (-1, "signal: killed")


async def test_remove_missing_is_not_an_error(backend):
    sb = await _sandbox(backend)
    assert await sb.files.remove("/nope") is None


async def test_calls_after_kill_raise_timeout(backend):
    sb = await _sandbox(backend)
    await sb.kill()
    with pytest.raises(TimeoutException, match="The sandbox was not found"):
        await sb.commands.run("echo x")
    with pytest.raises(TimeoutException):
        await sb.files.read("/tmp/x")


async def test_lifetime_expiry_then_calls_raise_timeout(backend):
    sb = await _sandbox(backend, timeout=0.02)
    await asyncio.sleep(0.1)
    with pytest.raises(TimeoutException):
        await sb.commands.run("echo x")


# ── lifetimes, network, pause/resume ──────────────────────────────────────


async def test_lifetime_goes_to_the_backend_with_its_action(backend):
    sb = await _sandbox(backend, timeout=600)
    assert backend.machines[sb.sandbox_id].lifetime == (600, "delete")
    await sb.set_timeout(30)
    assert backend.machines[sb.sandbox_id].lifetime == (30, "delete")
    paused = await _sandbox(backend, timeout=60, lifecycle={"on_timeout": "pause"})
    assert backend.machines[paused.sandbox_id].lifetime == (60, "suspend")
    default = await _sandbox(backend)
    assert backend.machines[default.sandbox_id].lifetime == (300, "delete")


async def test_set_timeout_by_id(backend):
    sb = await _sandbox(backend)
    await AsyncSandbox.set_timeout(sb.sandbox_id, 42)
    assert backend.machines[sb.sandbox_id].lifetime == (42, "delete")


async def test_pause_lifetime_then_auto_resume(backend):
    sb = await _sandbox(
        backend, timeout=0.02, lifecycle={"on_timeout": "pause", "auto_resume": True}
    )
    await asyncio.sleep(0.08)
    m = backend.machines[sb.sandbox_id]
    assert m.suspended and not m.deleted
    sb._paused = True  # what get_info would report after the daemon paused it
    assert (await sb.commands.run("echo back")).stdout == "back\n"
    assert not m.suspended


async def test_bad_lifecycle_is_refused(backend):
    with pytest.raises(e2b.InvalidArgumentException):
        await _sandbox(backend, lifecycle={"on_timeout": "explode"})


async def test_no_internet_is_network_none(backend):
    off = await _sandbox(backend, allow_internet_access=False)
    on = await _sandbox(backend)
    deny_all = await _sandbox(backend, network={"deny_out": [e2b.ALL_TRAFFIC]})
    assert backend.machines[off.sandbox_id].network == "none"
    assert backend.machines[on.sandbox_id].network is None
    assert backend.machines[deny_all.sandbox_id].network == "none"
    info = await off.get_info()
    assert info.allow_internet_access is False


async def test_ip_rules_become_egress_rules_in_the_space(backend):
    sb = await _sandbox(
        backend, network={"allow_out": ["1.2.3.4", "10.0.0.0/8"], "deny_out": [e2b.ALL_TRAFFIC]}
    )
    m = backend.machines[sb.sandbox_id]
    assert m.network is None
    assert m.egress == {"allow": ["1.2.3.4", "10.0.0.0/8"], "deny": ["0.0.0.0/0"]}
    info = await sb.get_info()
    assert info.network == {
        "allow_out": ["1.2.3.4", "10.0.0.0/8"],
        "deny_out": ["0.0.0.0/0"],
        "allow_public_traffic": True,
    }
    assert info.allow_internet_access is False
    only_deny = await _sandbox(backend, network={"deny_out": ["8.8.8.8"]})
    assert backend.machines[only_deny.sandbox_id].egress == {"allow": [], "deny": ["8.8.8.8"]}


async def test_proxy_rules_are_refused_not_dropped(backend):
    with pytest.raises(e2b.InvalidArgumentException, match="TLS-intercepting proxy"):
        await _sandbox(
            backend, network={"rules": {"api.x.com": [{"transform": {"headers": {"a": "b"}}}]}}
        )
    with pytest.raises(e2b.InvalidArgumentException, match="TLS-intercepting proxy"):
        await _sandbox(backend, network={"egress_proxy": {"address": "p:1080"}})


async def test_pause_and_connect(backend):
    sb = await _sandbox(backend)
    assert await sb.pause() is True
    assert not await sb.is_running()
    await AsyncSandbox.connect(sb.sandbox_id)
    assert not backend.machines[sb.sandbox_id].suspended


async def test_pause_modes_and_cold_resume(backend):
    """e2b 2.56: pause(mode=...), the deprecated keep_memory, connect(on_resume=...)."""
    full = await _sandbox(backend)
    await full.pause()
    await AsyncSandbox.connect(full.sandbox_id)
    assert backend.machines[full.sandbox_id].reboots == 0  # memory restored

    for kwargs in ({"mode": "filesystem"}, {"keep_memory": False}):
        sb = await _sandbox(backend)
        await sb.pause(**kwargs)
        await AsyncSandbox.connect(sb.sandbox_id)
        assert backend.machines[sb.sandbox_id].reboots == 1  # cold boot from disk

    kept = await _sandbox(backend)
    await kept.pause(mode="full")
    await AsyncSandbox.connect(kept.sandbox_id)
    assert backend.machines[kept.sandbox_id].reboots == 0

    again = await _sandbox(backend)
    await again.pause(mode="filesystem")
    await again.connect()  # on the instance too
    assert backend.machines[again.sandbox_id].reboots == 1

    with pytest.raises(e2b.InvalidArgumentException, match="not both"):
        await (await _sandbox(backend)).pause(keep_memory=True, mode="full")
    with pytest.raises(e2b.InvalidArgumentException, match="full, filesystem"):
        await (await _sandbox(backend)).pause(mode="disk")


async def test_connect_on_resume_reboot(backend):
    sb = await _sandbox(backend)
    await sb.pause()
    await AsyncSandbox.connect(sb.sandbox_id, on_resume="reboot")
    assert backend.machines[sb.sandbox_id].reboots == 1
    running = await _sandbox(backend)
    await AsyncSandbox.connect(running.sandbox_id, on_resume="reboot")
    assert backend.machines[running.sandbox_id].reboots == 0  # only a paused one reboots
    with pytest.raises(e2b.InvalidArgumentException, match="restore, reboot"):
        await AsyncSandbox.connect(sb.sandbox_id, on_resume="now")


async def test_lifecycle_object_form_and_its_rules(backend):
    sb = await _sandbox(
        backend, lifecycle={"on_timeout": {"action": "pause", "mode": "filesystem"}}
    )
    assert backend.machines[sb.sandbox_id].lifetime == (300, "suspend")
    assert backend.records[sb.sandbox_id]["resume"] == "cold"
    assert (await sb.get_info()).lifecycle["on_timeout"] == "pause"
    plain = await _sandbox(backend, lifecycle={"on_timeout": {"action": "pause"}})
    assert "resume" not in backend.records[plain.sandbox_id]
    for bad, text in (
        ({"on_timeout": {"action": "kill", "mode": "full"}}, "only allowed"),
        ({"on_timeout": "kill", "auto_resume": True}, "can only be True"),
        (
            {"on_timeout": {"action": "pause", "mode": "filesystem"}, "auto_resume": True},
            "filesystem",
        ),
        ({"on_timeout": {"action": "boom"}}, 'on_timeout\\["action"\\]'),
    ):
        with pytest.raises(e2b.InvalidArgumentException, match=text):
            await _sandbox(backend, lifecycle=bad)


async def test_snapshot_modes_and_none_defaults(backend):
    sb = await _sandbox(backend, secure=None, allow_internet_access=None)
    assert backend.machines[sb.sandbox_id].network is None  # None is e2b's "internet on"
    await sb.create_snapshot("warm", mode="filesystem")
    await sb.create_snapshot("hot")
    assert backend.machines[sb.sandbox_id].cold_snapshots == {"warm": True, "hot": False}
    with pytest.raises(e2b.InvalidArgumentException):
        await sb.create_snapshot("x", mode="memory")


async def test_snapshots_list_and_delete(backend):
    sb, other = await _sandbox(backend), await _sandbox(backend)
    await sb.create_snapshot("one")
    await other.create_snapshot("two")
    every = await AsyncSandbox.list_snapshots().next_items()
    assert sorted(i.snapshot_id for i in every) == ["one", "two"]
    mine = await sb.list_snapshots().next_items()
    assert [i.snapshot_id for i in mine] == ["one"]
    assert [i.snapshot_id for i in await AsyncSandbox.list_snapshots(name="two").next_items()] == [
        "two"
    ]
    assert await AsyncSandbox.delete_snapshot("one") is True
    assert await AsyncSandbox.delete_snapshot("one") is False
    assert [i.snapshot_id for i in await AsyncSandbox.list_snapshots().next_items()] == ["two"]


async def test_fork_a_running_sandbox(backend):
    sb = await _sandbox(backend, metadata={"k": "v"})
    await sb.files.write("/tmp/x", "hello")
    forks = await sb.fork(count=3, timeout=90)
    assert len(forks) == 3 and all(isinstance(f, AsyncSandbox) for f in forks)
    assert len({f.sandbox_id for f in forks}) == 3
    assert backend.machines[forks[0].sandbox_id].lifetime == (90, "delete")
    assert (await forks[0].files.read("/tmp/x")) == "hello"
    assert forks[0].metadata == {"k": "v"}
    by_id = await AsyncSandbox.fork(sb.sandbox_id)  # the class form, count defaults to 1
    assert len(by_id) == 1
    with pytest.raises(e2b.InvalidArgumentException):
        await sb.fork(count=21)
    # a fork that cannot start is reported in its slot, not raised
    await sb.pause()
    again = await AsyncSandbox.connect(sb.sandbox_id)
    backend.machines[again.sandbox_id].suspended = True
    got = await again.fork(count=2)
    assert len(got) == 2 and all(isinstance(r, Exception) for r in got)


async def test_list_order(backend):
    a, b = await _sandbox(backend), await _sandbox(backend)
    for i, sb in enumerate((a, b)):
        backend.records[sb.sandbox_id]["started"] = i
    got = [i.sandbox_id for i in await AsyncSandbox.list(order="desc").next_items()]
    assert sorted(got) == sorted([a.sandbox_id, b.sandbox_id])
    # like e2b, the order is checked when the page is fetched
    with pytest.raises(e2b.InvalidArgumentException):
        await AsyncSandbox.list(order="sideways").next_items()


# ── files: stream, owners, metadata, rename, watch ─────────────────────────


async def test_stream_cancels_on_early_close(backend):
    sb = await _sandbox(backend)
    await sb.files.write("/tmp/s", b"x" * (300 * 1024))
    reader = await sb.files.read("/tmp/s", format="stream")
    first = await reader.__anext__()
    assert len(first) == 64 * 1024
    await reader.aclose()
    assert reader._d.cancelled


async def test_stream_of_missing_file_fails_at_once(backend):
    sb = await _sandbox(backend)
    with pytest.raises(FileNotFoundException):
        await sb.files.read("/nope", format="stream")


async def test_write_gives_the_file_to_the_user(backend):
    sb = await _sandbox(backend)
    m = backend.machines[sb.sandbox_id]
    await sb.files.write("/tmp/as-user", "1")  # default user: cua
    await sb.files.write("/tmp/as-root", "1", user="root")
    assert m.owners["/tmp/as-user"] == "cua"
    assert m.owners["/tmp/as-root"] == "root"
    info = await sb.files.get_info("/tmp/as-user")
    assert (info.owner, info.group) == ("cua", "cua")


async def test_write_metadata_round_trip(backend):
    sb = await _sandbox(backend)
    w = await sb.files.write("/tmp/m", "x", metadata={"Kind": "input"})
    assert w.metadata == {"kind": "input"}
    assert (await sb.files.get_info("/tmp/m")).metadata == {"kind": "input"}


async def test_rename(backend):
    sb = await _sandbox(backend)
    await sb.files.write("/tmp/a", "1")
    info = await sb.files.rename("/tmp/a", "/tmp/b")
    assert info.path == "/tmp/b"
    assert await sb.files.read("/tmp/b") == "1"
    with pytest.raises(FileNotFoundException):
        await sb.files.rename("/tmp/missing", "/tmp/c")


async def test_watch_dir(backend):
    sb = await _sandbox(backend)
    seen = []
    handle = await sb.files.watch_dir("/w", on_event=seen.append, timeout=0)
    await sb.files.write("/w/new.txt", "1")
    for _ in range(50):
        if seen:
            break
        await asyncio.sleep(0.05)
    await handle.stop()
    assert seen and seen[0].name == "new.txt"
    assert seen[0].type == e2b.FilesystemEventType.WRITE


# ── pty, info, list ────────────────────────────────────────────────────────


async def test_pty(backend):
    sb = await _sandbox(backend)
    out = []
    h = await sb.pty.create(e2b.PtySize(rows=24, cols=80), on_data=out.append)
    spawned = backend.machines[sb.sandbox_id].spawned[-1]
    assert spawned["argv"] == ["/bin/bash", "-i", "-l"] and spawned["pty"] == (80, 24)
    assert spawned["env"]["TERM"] == "xterm-256color"
    await sb.pty.send_stdin(h.pid, b"ls\r")
    await sb.pty.resize(h.pid, e2b.PtySize(rows=40, cols=120))
    for _ in range(20):
        if out:
            break
        await asyncio.sleep(0.01)
    assert out == [b"ls\r"]
    assert backend.machines[sb.sandbox_id].procs[h.pid].size == (120, 40)
    assert await sb.pty.kill(h.pid) is True


async def test_get_info_and_metrics(backend):
    sb = await _sandbox(backend, metadata={"run": "7"})
    info = await sb.get_info()
    assert info.sandbox_id == sb.sandbox_id and info.template_id == "world-1"
    assert info.metadata == {"run": "7"} and info.state == "running"
    assert info.cpu_count == 2 and info.memory_mb == 3906
    [m] = await sb.get_metrics()
    assert isinstance(m, e2b.SandboxMetrics) and m.cpu_used_pct == 12.5
    assert m.mem_used == 1024000000 and m.disk_total == 1000


async def test_list_with_metadata_query(backend):
    a = await _sandbox(backend, metadata={"run": "a"})
    await _sandbox(backend, metadata={"run": "b"})
    pager = AsyncSandbox.list(query=e2b.SandboxQuery(metadata={"run": "a"}))
    assert pager.has_next
    items = await pager.next_items()
    assert [i.sandbox_id for i in items] == [a.sandbox_id]
    assert not pager.has_next
    await a.kill()
    assert a.sandbox_id not in [i.sandbox_id for i in await AsyncSandbox.list().next_items()]


def test_sync_pty_watch_and_list(backend):
    sb = Sandbox.create(template="world-1")
    w = sb.files.watch_dir("/w")
    sb.files.write("/w/x", "1")
    events = w.get_new_events()
    assert [e.name for e in events] == ["x"]
    w.stop()
    assert [i.sandbox_id for i in Sandbox.list().next_items()] == [sb.sandbox_id]
    h = sb.pty.create(e2b.PtySize(rows=24, cols=80))
    assert sb.pty.kill(h.pid) is True
    sb.kill()


# ── git ────────────────────────────────────────────────────────────────────


def test_git_status_parsing():
    from e2b.sandbox_async.git import parse_status

    st = parse_status(
        "## main...origin/main [ahead 2, behind 1]\n"
        " M changed.py\n"
        "A  added.py\n"
        "R  old.py -> new.py\n"
        "UU both.py\n"
        "?? new.txt\n"
    )
    assert (st.current_branch, st.upstream, st.ahead, st.behind, st.detached) == (
        "main",
        "origin/main",
        2,
        1,
        False,
    )
    by = {f.name: f for f in st.file_status}
    assert by["changed.py"].status == "modified" and not by["changed.py"].staged
    assert by["added.py"].staged and by["new.py"].renamed_from == "old.py"
    assert by["both.py"].status == "conflict" and st.has_conflicts
    assert st.untracked_count == 1 and st.staged_count == 3
    fresh = parse_status("## No commits yet on main\n?? a.txt\n")
    assert fresh.current_branch == "main" and fresh.has_untracked
    assert parse_status("## HEAD (no branch)\n").detached


async def test_git_runs_git_commands(backend):
    sb = await _sandbox(backend)
    with pytest.raises(CommandExitException):  # the fake has no git
        await sb.git.init("/r", initial_branch="main")
    argv = backend.machines[sb.sandbox_id].spawned[-1]["argv"]
    assert argv[-1] == "git init --initial-branch main /r"
    assert backend.machines[sb.sandbox_id].spawned[-1]["env"]["GIT_TERMINAL_PROMPT"] == "0"


# ── templates: a recipe built into a snapshot, or a container image ───────


async def test_template_recipe_runs_in_a_vm_and_is_snapshotted(backend, tmp_path):
    backend.permissive = True
    (tmp_path / "app").mkdir()
    (tmp_path / "app" / "main.py").write_text("print('hi')")
    logs = []
    t = (
        e2b.Template(file_context_path=tmp_path)
        .from_base_image()
        .set_envs({"MODE": "rl"})
        .set_workdir("/srv")
        .copy("app", "/srv/app")
        .run_cmd(["echo one", "echo two"])
        .apt_install(["git"], no_install_recommends=True)
        .set_start_cmd("serve /srv/app/main.py", e2b.wait_for_port(8080))
    )
    info = await e2b.AsyncTemplate.build(t, "world-7", on_build_logs=logs.append)
    assert (info.name, info.template_id) == ("world-7", "world-7")
    snapshot = f"world-7-{info.build_id}"
    builder = next(m for m in backend.machines.values() if snapshot in m.cold_snapshots)
    assert builder.deleted  # the builder is gone, the snapshot stays
    ran = [s["argv"][-1] for s in builder.spawned]
    assert "echo one && echo two" in ran
    assert any("apt-get install -y --no-install-recommends git" in c for c in ran)
    assert any("serve /srv/app/main.py" in c for c in ran)
    assert builder.files["/srv/app/main.py"] == b"print('hi')"
    assert backend.templates["world-7"]["snapshot"] == snapshot
    assert backend.templates["world-7:default"]["snapshot"] == snapshot
    assert await e2b.AsyncTemplate.exists("world-7") is True
    assert any("step 3/" in str(e) for e in logs)
    status = await e2b.AsyncTemplate.get_build_status(info)
    assert status.status == "ready"
    assert await e2b.AsyncTemplate.delete("world-7") is True
    assert await e2b.AsyncTemplate.exists("world-7") is False


async def test_template_from_an_oci_image_is_refused_with_the_way_out(backend):
    t = e2b.Template().from_python_image("3.12").run_cmd("pip install requests")
    with pytest.raises(e2b.BuildException, match="cua images build"):
        await e2b.AsyncTemplate.build(t, "web")
    assert backend.template("web") is None
    ok = e2b.Template().from_image("linux").run_cmd("echo hi")
    backend.permissive = True
    await e2b.AsyncTemplate.build(ok, "fine")
    assert backend.template("fine")["kind"] == "snapshot"


async def test_template_dockerfile_and_exports(backend, tmp_path):
    df = 'FROM python:3.12\nENV A=1\nWORKDIR /w\nRUN pip install x \\\n  && echo ok\nCMD ["python", "-m", "app"]\n'
    t = e2b.Template().from_dockerfile(df)
    text = e2b.Template.to_dockerfile(t)
    assert text.splitlines()[0] == "FROM python:3.12"
    assert "RUN pip install x   && echo ok" in text and "ENTRYPOINT python -m app" in text
    assert json.loads(e2b.Template.to_json(t))["startCmd"] == "python -m app"
    with pytest.raises(e2b.DockerfileSyntaxError, match="FROM"):
        e2b.Template().from_dockerfile("RUN echo no from")
    with pytest.raises(TemplateException, match="Name must be provided"):
        await e2b.AsyncTemplate.build(t)  # no name
    with pytest.raises(e2b.BuildException, match="mcp-gateway"):
        t.add_mcp_server("github")


async def test_a_failed_template_build_is_a_build_exception(backend):
    t = e2b.Template().from_base_image().run_cmd("fail 3")
    with pytest.raises(e2b.BuildException):
        await e2b.AsyncTemplate.build(t, "bad")
    assert backend.template("bad") is None


# ── volumes ───────────────────────────────────────────────────────────────


async def test_volume_files_roundtrip_on_cua_volume(backend):
    v = await e2b.AsyncVolume.create("data")
    assert (v.volume_id, v.name) == ("data", "data")
    assert [i.name for i in await e2b.AsyncVolume.list()] == ["data"]
    st = await v.write_file("/a/b.txt", "hello", mode=0o600, force=True)
    assert (st.name, st.size, st.mode, st.type) == ("b.txt", 5, 0o600, e2b.VolumeFileType.FILE)
    assert await v.read_file("/a/b.txt") == "hello"
    assert await v.read_file("a/b.txt", format="bytes") == b"hello"
    chunks = [c async for c in await v.read_file("/a/b.txt", format="stream")]
    assert b"".join(chunks) == b"hello"
    assert await v.exists("/a") and await v.exists("/a/b.txt") and not await v.exists("/a/c")
    await v.make_dir("/empty")
    assert await v.exists("/empty")
    assert (await v.update_metadata("/a/b.txt", uid=7)).uid == 7
    assert "e2b-volumes/data/a/b.txt" in backend.volume.files  # a folder of Cua Volume
    with pytest.raises(e2b.VolumePathNotFoundException):
        await v.read_file("/nope")
    await v.remove("/a")
    assert not await v.exists("/a/b.txt")
    again = await e2b.AsyncVolume.connect("data")
    assert again.name == "data"
    info = await e2b.AsyncVolume.get_info("data")
    assert info.volume_id == "data"
    assert await e2b.AsyncVolume.destroy("data") is True
    assert await e2b.AsyncVolume.destroy("data") is False
    with pytest.raises(e2b.VolumeNotFoundException):
        await e2b.AsyncVolume.connect("data")


def test_volume_sync_wrapper(backend):
    v = e2b.Volume.create("sync")
    v.write_file("/x", b"1")
    assert v.read_file("/x", format="bytes") == b"1"
    assert b"".join(v.read_file("/x", format="stream")) == b"1"
    assert [i.volume_id for i in e2b.Volume.list()] == ["sync"]
    assert e2b.Volume.destroy("sync") is True


async def test_volume_mounts_link_the_volume_folder_when_mounted(backend):
    backend.permissive = True
    v = await e2b.AsyncVolume.create("data")
    backend.volume_mounted = True
    sb = await AsyncSandbox.create(template="world-1", volume_mounts={"/mnt/data": v})
    ran = [s["argv"][-1] for s in backend.machines[sb.sandbox_id].spawned]
    assert any("ln -sfn /volume/e2b-volumes/data /mnt/data" in c for c in ran)
    backend.volume_mounted = False
    before = len(backend.machines)
    with pytest.raises(e2b.InvalidArgumentException, match="Cua Volume mounted"):
        await AsyncSandbox.create(template="world-1", volume_mounts={"/mnt/data": "data"})
    assert all(m.deleted for m in list(backend.machines.values())[before:])  # not left behind


async def test_mcp_starts_the_gateway_as_e2b_does_and_iam_is_refused(backend):
    backend.permissive = True
    sb = await _sandbox(backend, mcp={"github": {"x": 1}})
    m = backend.machines[sb.sandbox_id]
    started = [c for c in m.spawned if "mcp-gateway" in c["argv"][-1]][0]
    assert started["user"] == "root" and started["env"]["GATEWAY_ACCESS_TOKEN"] == sb._mcp_token
    assert started["argv"][-1].startswith("mcp-gateway --config ")
    assert sb.get_mcp_url().startswith("http://") and sb.get_mcp_url().endswith("/mcp")
    backend.permissive = False
    with pytest.raises(e2b.SandboxException, match="Failed to start MCP gateway"):
        await _sandbox(backend, mcp={"github": {}})
    assert all(m.deleted for m in list(backend.machines.values())[1:])  # no sandbox left behind
    with pytest.raises(e2b.InvalidArgumentException, match="Keyvault"):
        await _sandbox(
            backend, iam={"tokens": {"aws": {"audience": "sts", "token_type": "JWT-SVID"}}}
        )

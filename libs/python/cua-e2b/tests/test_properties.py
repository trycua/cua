"""Property and fuzz tests of the translation layer (hypothesis): whatever the
input, the shim hands the backend what e2b's contract says, maps every
failure to an e2b exception, and loses no bytes."""

from __future__ import annotations

import asyncio
import io
import string

import pytest

pytest.importorskip("hypothesis")  # installed by the e2b-conformance workflow's hermetic job

from e2b import (
    AsyncSandbox,
    CommandExitException,
    InvalidArgumentException,
    SandboxException,
    TimeoutException,
)
from e2b._backend import ProcEvent, set_backend
from e2b._errors import map_error
from e2b.sandbox_async.commands.command import _timeout_ms, guest_user
from e2b.sandbox_async.commands.command_handle import AsyncCommandHandle
from e2b.sandbox_async.main import _resolve_lifecycle, _snapshot_mode
from fake_backend import FakeBackend
from hypothesis import HealthCheck, given, settings
from hypothesis import strategies as st

FAST = settings(max_examples=60, deadline=None, suppress_health_check=[HealthCheck.too_slow])


def run(coro):
    return asyncio.run(coro)


# -- timeouts and users --------------------------------------------------------


@given(st.one_of(st.none(), st.floats(min_value=0, max_value=1e12, allow_nan=False)))
def test_timeout_ms_is_none_for_no_limit_else_a_bounded_int(t):
    ms = _timeout_ms(t)
    if not t:
        assert ms is None
    else:
        assert isinstance(ms, int) and 0 <= ms <= 2**32 - 1
        if t < 4e6:
            assert abs(ms - t * 1000) <= 1


@given(st.text(max_size=20))
def test_guest_user_only_collapses_the_default_names(u):
    assert guest_user(u) == (None if u in ("", "user") else u)


# -- error mapping ---------------------------------------------------------------

CUA_ERRORS = [
    "NotFound",
    "ImageNotPublished",
    "Timeout",
    "InvalidArgument",
    "InvalidPlacement",
    "AmbiguousSandbox",
    "PermissionDenied",
    "Unauthenticated",
    "InsufficientDisk",
    "Transport",
    "Closed",
    "DaemonNotRunning",
    "Runtime",
    "Env",
    "Internal",
    "SomethingNew",
]


@given(
    st.sampled_from(CUA_ERRORS),
    st.text(max_size=80),
    st.booleans(),
    st.booleans(),
    st.booleans(),
)
def test_every_cua_error_becomes_an_e2b_exception_that_keeps_the_message(
    name, msg, file, template, process
):
    base = type("CuaError", (Exception,), {})
    err = type(name, (base,), {})(msg)
    out = map_error(err, file=file, template=template, process=process)
    assert isinstance(out, (SandboxException, Exception))
    assert type(out).__module__.startswith("e2b")
    assert msg in str(out)
    assert map_error(out) is out  # mapping is idempotent


@given(
    st.dictionaries(
        st.text(max_size=8), st.one_of(st.none(), st.text(max_size=8), st.booleans(), st.integers())
    )
)
def test_lifecycle_validation_never_raises_anything_but_invalid_argument(lc):
    try:
        out = _resolve_lifecycle(lc)
    except InvalidArgumentException:
        return
    assert set(out) == {"on_timeout", "mode", "auto_resume"}
    assert out["on_timeout"] in ("pause", "kill")


@given(st.one_of(st.none(), st.text(max_size=12)), st.one_of(st.none(), st.booleans()))
def test_snapshot_mode_validation(mode, keep):
    try:
        out = _snapshot_mode("mode", mode, keep)
    except InvalidArgumentException:
        return
    assert out in (None, "full", "filesystem")
    if mode is not None:
        assert keep is None and out == mode


# -- the stream pump -------------------------------------------------------------


class ScriptedProcess:
    pid = 7

    def __init__(self, events):
        self._events = list(events)

    async def next_event(self):
        return self._events.pop(0) if self._events else None

    async def kill(self):
        return True

    async def detach(self):
        pass


def cut(data: bytes, points):
    pts = sorted({p % (len(data) + 1) for p in points})
    out, last = [], 0
    for p in pts + [len(data)]:
        out.append(data[last:p])
        last = p
    return [c for c in out if c]


@given(
    st.binary(max_size=300),
    st.lists(st.integers(min_value=0, max_value=300), max_size=12),
    st.binary(max_size=100),
)
def test_output_chunks_lose_nothing_and_decode_like_one_piece(out_bytes, points, err_bytes):
    events = [ProcEvent(kind="stdout", data=c) for c in cut(out_bytes, points)]
    events += [ProcEvent(kind="stderr", data=c) for c in cut(err_bytes, points)]
    events.append(ProcEvent(kind="exit", exit_code=0))

    async def go():
        seen = []
        h = AsyncCommandHandle(pid=7, process=ScriptedProcess(events), on_stdout=seen.append)
        r = await h.wait()
        return r, "".join(seen)

    r, callbacks = run(go())
    assert r.stdout == out_bytes.decode("utf-8", "replace") == callbacks
    assert r.stderr == err_bytes.decode("utf-8", "replace")


@given(st.integers(min_value=1, max_value=255))
def test_nonzero_exit_codes_raise_with_e2bs_error_text(code):
    async def go():
        h = AsyncCommandHandle(
            pid=1, process=ScriptedProcess([ProcEvent(kind="exit", exit_code=code)])
        )
        await h.wait()

    with pytest.raises(CommandExitException) as info:
        run(go())
    assert (info.value.exit_code, info.value.error) == (code, f"exit status {code}")
    assert str(info.value).startswith(f"Command exited with code {code} and error:")


@given(st.sampled_from(["kill", "term", "int", "hup", "quit", "abrt", "segv", "pipe", "usr1"]))
def test_death_by_signal_is_exit_minus_one_with_a_go_style_error(sig):
    async def go():
        h = AsyncCommandHandle(
            pid=1, process=ScriptedProcess([ProcEvent(kind="exit", exit_code=None, signal=sig)])
        )
        await h.wait()

    with pytest.raises(CommandExitException) as info:
        run(go())
    assert info.value.exit_code == -1 and info.value.error.startswith("signal: ")


def test_a_timed_out_command_raises_timeout_exception():
    async def go():
        h = AsyncCommandHandle(
            pid=1,
            process=ScriptedProcess(
                [ProcEvent(kind="exit", exit_code=None, signal="kill", timed_out=True)]
            ),
        )
        await h.wait()

    with pytest.raises(TimeoutException):
        run(go())


# -- the sandbox surface over the in-memory backend -----------------------------------

paths = st.text(alphabet=string.ascii_letters + string.digits + " _-.é中", min_size=1, max_size=24)


@pytest.fixture
def box():
    b = FakeBackend()
    set_backend(b)
    yield b
    set_backend(None)


@FAST
@given(paths, st.binary(max_size=4096))
def test_bytes_round_trip_for_any_path_and_content(name, data):
    async def go():
        b = FakeBackend()
        set_backend(b)
        sb = await AsyncSandbox.create(template="w")
        p = f"/tmp/{name}"
        info = await sb.files.write(p, data)
        assert info.path == p and info.name == name
        assert bytes(await sb.files.read(p, format="bytes")) == data
        assert await sb.files.exists(p)
        set_backend(None)

    run(go())


@FAST
@given(st.text(max_size=300))
def test_text_round_trips_through_utf8(text):
    raw = text.encode()

    async def go():
        set_backend(FakeBackend())
        sb = await AsyncSandbox.create(template="w")
        await sb.files.write("/tmp/t", text)
        assert await sb.files.read("/tmp/t") == raw.decode()
        await sb.files.write("/tmp/t2", io.BytesIO(raw))
        assert await sb.files.read("/tmp/t2") == raw.decode()
        set_backend(None)

    run(go())


@FAST
@given(
    st.dictionaries(
        st.text(alphabet=string.ascii_uppercase + "_", min_size=1, max_size=6),
        st.text(max_size=10),
        max_size=4,
    ),
    st.dictionaries(
        st.text(alphabet=string.ascii_uppercase + "_", min_size=1, max_size=6),
        st.text(max_size=10),
        max_size=4,
    ),
    st.text(min_size=1, max_size=40).filter(lambda s: s.strip()),
)
def test_command_line_and_env_reach_the_backend_verbatim(sb_envs, call_envs, cmd):
    async def go():
        b = FakeBackend()
        b.permissive = True
        set_backend(b)
        sb = await AsyncSandbox.create(template="w", envs=sb_envs)
        b.machines[sb.sandbox_id].dirs.add("/w")
        await sb.commands.run(cmd, envs=call_envs, cwd="/w", user="root")
        spawned = b.machines[sb.sandbox_id].spawned[-1]
        assert spawned["argv"] == ["/bin/bash", "-l", "-c", cmd]
        assert spawned["env"] == {**sb_envs, **call_envs}
        assert (spawned["cwd"], spawned["user"]) == ("/w", "root")
        set_backend(None)

    run(go())

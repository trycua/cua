"""The E2B calls an RL rollout harness makes, as a hermetic contract test
(no code of any customer repo: only the call shapes the migration plan
lists): sandboxes created from a baked snapshot with a long timeout and
optionally no internet, a background supervisor streamed through a callback,
uploads of bytes and of open binary files as root, a streamed download used
as an async context manager, port hosts for tool servers, and kill with a
request timeout; plus the exception classes such code catches by name."""

from __future__ import annotations

import asyncio
import io

import pytest
from e2b import (
    AsyncSandbox,
    AuthenticationException,
    CommandExitException,
    InvalidArgumentException,
    NotEnoughSpaceException,
    SandboxException,
    TimeoutException,
)
from e2b._backend import set_backend
from fake_backend import FakeBackend


@pytest.fixture
def backend():
    b = FakeBackend()
    b.permissive = True
    set_backend(b)
    yield b
    set_backend(None)


async def test_create_from_a_snapshot_with_the_rollout_options(backend):
    sb = await AsyncSandbox.create(
        template="baked-world", timeout=7200, allow_internet_access=False, request_timeout=30
    )
    assert backend.machines[sb.sandbox_id].network == "none"
    assert backend.machines[sb.sandbox_id].lifetime[0] == 7200
    sb2 = await AsyncSandbox.create(template="baked-world", allow_internet_access=True)
    assert backend.machines[sb2.sandbox_id].network is None


async def test_background_supervisor_streams_a_reply_then_disconnects(backend):
    sb = await AsyncSandbox.create(template="w")
    loop = asyncio.get_running_loop()
    reply = loop.create_future()

    def receive(text):
        if not reply.done():
            reply.set_result(text.partition("\n")[0])

    launch = asyncio.create_task(
        sb.commands.run("reply", background=True, timeout=0, user="root", on_stdout=receive)
    )
    handle = await asyncio.shield(launch)
    waiter = asyncio.create_task(handle.wait())
    await asyncio.wait((reply, waiter), return_when=asyncio.FIRST_COMPLETED)
    assert reply.result() == '{"ok": true}'
    await handle.disconnect()
    await asyncio.gather(waiter, return_exceptions=True)
    assert handle.exit_code is None  # still running in the sandbox
    await sb.commands.kill(handle.pid)


async def test_uploads_of_bytes_and_open_binary_files_as_root(backend):
    sb = await AsyncSandbox.create(template="w")
    await sb.files.write("/tmp/a", b"\x00\x01" * 1000, user="root", request_timeout=120)
    f = io.BytesIO(b"seekable")
    pos = f.tell()
    await sb.files.write("/tmp/b", f, user="root", request_timeout=120)
    f.seek(pos)  # a retry rewinds and writes again
    await sb.files.write("/tmp/b", f, user="root", request_timeout=120)
    m = backend.machines[sb.sandbox_id]
    assert m.files["/tmp/a"] == b"\x00\x01" * 1000 and m.files["/tmp/b"] == b"seekable"
    assert m.owners["/tmp/b"] == "root"
    await sb.files.remove("/tmp/b", user="root", request_timeout=30)
    await sb.files.remove("/tmp/b", user="root", request_timeout=30)  # missing: no error


async def test_streamed_download_as_an_async_context_manager(backend):
    sb = await AsyncSandbox.create(template="w")
    data = bytes(range(256)) * 1000
    await sb.files.write("/tmp/arch", data)
    got = bytearray()
    async with await sb.files.read(
        "/tmp/arch", format="stream", user="root", request_timeout=60
    ) as s:
        async for chunk in s:
            got += chunk
    assert bytes(got) == data


async def test_tool_server_hosts_and_kill_with_a_request_timeout(backend):
    sb = await AsyncSandbox.create(template="w")
    host = sb.get_host(8765)
    assert ":" in host and host == sb.get_host(8765)
    assert await sb.kill(request_timeout=15) is True


async def test_exception_classes_callers_branch_on(backend):
    sb = await AsyncSandbox.create(template="w")
    with pytest.raises(CommandExitException) as e:
        await sb.commands.run("fail 2", user="root", timeout=15)
    assert e.value.exit_code == 2 and isinstance(e.value, SandboxException)
    for cls in (
        AuthenticationException,
        NotEnoughSpaceException,
        InvalidArgumentException,
        TimeoutException,
    ):
        assert issubclass(cls, Exception)
    assert issubclass(NotEnoughSpaceException, SandboxException)
    assert issubclass(TimeoutException, SandboxException)


async def test_256_sibling_sandboxes_from_one_snapshot(backend):
    """32 workers x 8 siblings: concurrent creates and kills stay independent."""

    async def worker(i):
        sbs = await asyncio.gather(*(AsyncSandbox.create(template="baked-world") for _ in range(8)))
        out = await asyncio.gather(*(s.commands.run(f"echo {i}-{k}") for k, s in enumerate(sbs)))
        await asyncio.gather(*(s.kill() for s in sbs))
        return [o.stdout.strip() for o in out]

    results = await asyncio.gather(*(worker(i) for i in range(32)))
    assert {r for rs in results for r in rs} == {f"{i}-{k}" for i in range(32) for k in range(8)}
    assert all(m.deleted for m in backend.machines.values())

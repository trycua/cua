"""cua-sandbox's public interfaces against the cua SDK's MockServer spacesd.

``cua-test-fixtures`` (``cargo build -p cua-daemon --features test-fixtures
--bin cua-test-fixtures`` in libs/cua) serves a MockServer cua-spacesd on
loopback; nothing here touches the host. This drives the real stack end to
end: Sandbox → interfaces → EnvTransport → cua SDK (UniFFI) → gRPC →
MockServer, so every JSON request EnvTransport builds (pointer, keyboard) is
also validated by the server's proto3 parser.
"""

from __future__ import annotations

import asyncio
import json
import os
import subprocess
from pathlib import Path

import pytest
from cua_sandbox import Sandbox, SpacesdNotAvailable

CUA_ROOT = Path(__file__).resolve().parents[3] / "cua"


def _fixtures_binary() -> Path | None:
    explicit = os.environ.get("CUA_TEST_FIXTURES")
    if explicit:
        return Path(explicit)
    for profile in ("debug", "release"):
        candidate = CUA_ROOT / "target" / profile / "cua-test-fixtures"
        if candidate.exists():
            return candidate
    return None


@pytest.fixture(scope="module")
def fixtures():
    binary = _fixtures_binary()
    if binary is None:
        pytest.skip("cua-test-fixtures is not built (see module docstring)")
    proc = subprocess.Popen([str(binary)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    try:
        line = proc.stdout.readline()
        if not line:
            raise RuntimeError("cua-test-fixtures exited before printing endpoints")
        yield json.loads(line)
    finally:
        proc.stdin.close()
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=10)


@pytest.fixture
async def sb(fixtures):
    sandbox = await Sandbox.connect(url=fixtures["env_url"], token=fixtures["env_token"])
    yield sandbox
    await sandbox.disconnect()


async def test_shell(sb):
    ok = await sb.shell.run("echo hi from mock")
    assert ok.success and ok.stdout == "hi from mock\n"
    failed = await sb.shell.run("fail 4")
    assert (failed.returncode, failed.success) == (4, False)


async def test_files_round_trip(sb):
    payload = bytes(range(256)) * 512  # 128 KiB, crosses the SDK's chunking
    await sb.files.write_bytes("/tmp/cua-sandbox/blob", payload)
    assert await sb.files.read_bytes("/tmp/cua-sandbox/blob") == payload
    assert await sb.files.size("/tmp/cua-sandbox/blob") == len(payload)
    assert await sb.files.exists("/tmp/cua-sandbox/blob")
    assert not await sb.files.exists("/tmp/cua-sandbox/missing")

    await sb.files.write_text("/tmp/cua-sandbox/note.txt", "héllo")
    assert await sb.files.read_text("/tmp/cua-sandbox/note.txt") == "héllo"

    local = Path(__file__).parent / "__init__.py"
    await sb.files.upload(local, "/tmp/cua-sandbox/uploaded.py")
    assert await sb.files.read_bytes("/tmp/cua-sandbox/uploaded.py") == local.read_bytes()


async def test_input_requests_are_valid_spacesd_rpcs(sb):
    await sb.mouse.click(10, 20)
    await sb.mouse.click(10, 20, button="middle")
    await sb.mouse.right_click(1, 1)
    await sb.mouse.double_click(2, 2)
    await sb.mouse.move(30, 40)
    await sb.mouse.scroll(30, 40, scroll_y=-2)
    await sb.mouse.mouse_down(5, 5)
    await sb.mouse.mouse_up(6, 6)
    await sb.mouse.drag(1, 1, 9, 9)
    await sb.mouse.drag(1, 1, 9, 9, button="right")
    await sb.keyboard.type("typed")
    await sb.keyboard.keypress("enter")
    await sb.keyboard.keypress(["ctrl", "a"])
    await sb.keyboard.key_down("shift")
    await sb.keyboard.key_up("shift")


async def test_clipboard_screen_and_environment(sb):
    await sb.clipboard.set("from cua-sandbox")
    assert await sb.clipboard.get() == "from cua-sandbox"
    png = await sb.screenshot()
    assert png.startswith(b"\x89PNG")
    width, height = await sb.get_dimensions()
    assert width > 0 and height > 0
    assert await sb.get_environment() in ("linux", "mac", "windows")


async def test_raw_env_client_escape_hatch(sb):
    env = await sb.spacesd()
    caps = await env.capabilities()
    assert caps.version
    assert (await env.call_json("SystemService/Health", "{}")).startswith("{")


async def test_terminal_session(sb):
    # The MockServer's `sleep` takes milliseconds: `sleep 30` exited before
    # `info` asked. 30 s outlives the test; `close` ends it.
    session = await sb.terminal.create(command="sleep 30000", cols=100, rows=30)
    assert session["pid"] > 0 and (session["cols"], session["rows"]) == (100, 30)
    assert await sb.terminal.info(session["pid"]) is not None
    assert await sb.terminal.close(session["pid"]) is True


async def test_bad_token_is_not_reported_as_a_missing_driver(fixtures):
    sandbox = await Sandbox.connect(url=fixtures["env_url"], token="wrong")
    with pytest.raises(Exception) as caught:
        await sandbox.shell.run("echo hi")
    assert not isinstance(caught.value, SpacesdNotAvailable)
    assert "nauthenticated" in type(caught.value).__name__ + str(caught.value)


async def test_nothing_listening_is_spacesd_not_available():
    import socket

    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        free_port = probe.getsockname()[1]
    sandbox = await Sandbox.connect(url=f"http://127.0.0.1:{free_port}")
    with pytest.raises(SpacesdNotAvailable):
        await sandbox.shell.run("echo hi")


async def test_port_forward_needs_the_driver_tunnel(sb):
    # The MockServer does not advertise "tunnel.forward": a clear refusal,
    # never a raw TCP guess at the URL's host.
    with pytest.raises(Exception, match="tunnel.forward"):
        await sb.tunnel.forward(1234)


async def test_port_forward_over_the_real_driver_tunnel(fixtures):
    """``spaces_url`` is a real cua-spacesd core: bytes cross its /tunnel
    WebSocket to a loopback echo server owned by this test."""

    async def echo(reader, writer):
        try:
            while data := await reader.read(65536):
                writer.write(data)
                await writer.drain()
        finally:
            writer.close()

    server = await asyncio.start_server(echo, "127.0.0.1", 0)
    port = server.sockets[0].getsockname()[1]
    sandbox = await Sandbox.connect(url=fixtures["spaces_url"], token=fixtures["spaces_token"])
    try:
        async with sandbox.tunnel.forward(port) as t:
            assert t.host == "127.0.0.1" and t.port != port

            async def round_trip(i: int) -> None:
                reader, writer = await asyncio.open_connection(t.host, t.port)
                data = bytes([i]) * 200_000
                writer.write(data)
                await writer.drain()
                got = await asyncio.wait_for(reader.readexactly(len(data)), 15)
                assert got == data
                writer.close()

            await asyncio.gather(*(round_trip(i) for i in range(4)))
    finally:
        await sandbox.disconnect()
        server.close()
        await server.wait_closed()

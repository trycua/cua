"""Space lifetimes, streamed downloads and tunnel forwards through the
generated binding, against the fixture's in-process cua-spacesd core (temp
guest HOME; the "guest" port forwarded to is a loopback socket this test
opens). No VMs; nothing touches the real home."""

from __future__ import annotations

import os
import socket
import threading
import time
from pathlib import Path

import pytest
from test_smoke import run

import cua


def echo_server():
    srv = socket.socket()
    srv.bind(("127.0.0.1", 0))
    srv.listen(1)

    def serve():
        conn, _ = srv.accept()
        with conn:
            conn.sendall(conn.recv(64))
        srv.close()

    threading.Thread(target=serve, daemon=True).start()
    return srv.getsockname()[1]


async def exercise(c, fx):
    spaces = c.spaces()
    info = await spaces.add(fx["spaces_url"], fx["spaces_token"], "py-lifetimes")

    # A lifetime: when it ends, and refusals.
    now = time.time()
    at = await spaces.set_lifetime(info.id, 60, None)
    assert now + 55 < at <= now + 62
    assert await spaces.set_lifetime(info.id, 30, "suspend") <= now + 32
    with pytest.raises(cua.CuaError.InvalidArgument):
        await spaces.set_lifetime(info.id, 1, "explode")
    with pytest.raises(cua.CuaError.NotFound):
        await spaces.set_lifetime("direct:127.0.0.1:9", 1, None)

    space = await spaces.space(info.id)
    env = space.spacesd()

    # A streamed download arrives in chunks, byte for byte.
    guest = str(Path(fx["spaces_guest_home"]) / "py" / "stream.bin")
    blob = os.urandom(3 * 1024 * 1024 + 17)
    await env.upload(guest, blob, None)
    d = await env.open_download(guest, None)
    chunks = []
    while (c_ := await d.next_chunk()) is not None:
        chunks.append(bytes(c_))
    assert b"".join(chunks) == blob and len(chunks) > 1
    missing = await env.open_download("/does/not/exist", None)
    with pytest.raises(cua.CuaError.NotFound):
        await missing.next_chunk()
    early = await env.open_download(guest, None)
    assert await early.next_chunk()
    early.cancel()

    # A TCP forward through cua-spacesd's tunnel reaches a guest port.
    port = echo_server()
    fwd = await env.forward_tcp(port, None)
    assert fwd.guest_port() == port and fwd.local_addr().startswith("127.0.0.1:")
    host, p = fwd.local_addr().rsplit(":", 1)
    with socket.create_connection((host, int(p)), timeout=10) as s:
        s.sendall(b"ping")
        assert s.recv(64) == b"ping"
    await fwd.stop()
    with pytest.raises(cua.CuaError.InvalidArgument):
        await env.forward_tcp(port, "not-an-address")

    # `network` is checked before anything is created.
    with pytest.raises(cua.CuaError.InvalidArgument):
        await spaces.create(cua.SpaceCreateOptions(on="local", network="sometimes"))

    await spaces.delete(info.id)


def test_lifetimes_streams_and_forwards(fixtures, tmp_path):
    c = cua.embedded(
        state_dir=str(tmp_path / "sbx"),
        spaces_home=str(tmp_path / "cua"),
    )
    run(exercise(c, fixtures))


def test_sandbox_lifetime_refuses_unknown_actions(fixtures, tmp_path):
    async def go():
        c = cua.embedded(state_dir=str(tmp_path / "sbx"))
        sb = await c.sandboxes().connect_url(fixtures["env_url"], fixtures["env_token"], "py-life")
        with pytest.raises(cua.CuaError.InvalidArgument):
            await sb.set_lifetime(10, "explode")
        await sb.delete()

    run(go())

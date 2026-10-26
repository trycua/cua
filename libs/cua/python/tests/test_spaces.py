"""Spaces through the generated binding, embedded and against a `cua daemon`.

The Space is the fixture's in-process cua-spacesd server core (temp
guest HOME/PATH, temp Downloads and teleport home, fake driver tools).
Teleport ships with Cua Spaces: an embedded runtime and the MIT `cua
daemon` report it missing, and the Cua Spaces daemon (`cua-spaces-cli
daemon`) serves it, reading a synthetic Firefox profile through a
side-effect-free host rooted at a temp directory. Nothing touches the real
home, apps or keychain.
"""

from __future__ import annotations

import os
import subprocess
import tempfile
from pathlib import Path

import pytest
from test_smoke import run, wait_for

import cua


class Approver(cua.TeleportApprover):
    def __init__(self, decision):
        self.decision = decision
        self.seen = []

    def approve(self, manifest):
        self.seen.append(manifest)
        return self.decision


def test_every_contract_tool_maps_to_a_generated_method():
    rows = cua.spaces_tool_methods()
    assert len(rows) == 115
    classes = {"Spaces": cua.Spaces, "Space": cua.Space}
    for row in rows:
        cls, method = row.method.split(".")
        assert hasattr(classes[cls], method), f"{row.tool} -> {row.method} missing"


async def exercise(c, fx, tmp_path: Path, teleport: bool = False):
    spaces = c.spaces()
    info = await spaces.add(fx["spaces_url"], fx["spaces_token"], "py-space")
    assert info.provider == "direct" and info.id.startswith("direct:")
    assert "driver" in info.features
    assert [s.id for s in await spaces.list()] == [info.id]
    assert (await spaces.resolve("py-space")).id == info.id
    with pytest.raises(cua.CuaError.Unauthenticated):
        await spaces.add(fx["spaces_url"], "wrong", None)

    space = await spaces.space(info.id)
    if os.name == "nt":
        # On a Windows host the fixture Space reports Windows: cmd.exe.
        out = await space.bash("echo hi& exit 3", None)
        assert (out.stdout.strip(), out.exit_code) == ("hi", 3), out
    else:
        out = await space.bash("echo hi; exit 3", None)
        assert (out.stdout, out.exit_code, out.rendered) == ("hi\n", 3, "hi\n[exit 3]")
    assert Path(await space.home()) == Path(fx["spaces_guest_home"])

    guest = str(Path(fx["spaces_guest_home"]) / "py" / "note.txt")
    report = await space.write(guest, b"from python")
    assert report.bytes == 11
    src = tmp_path / "drop.txt"
    src.write_text("dropped")
    sent = await space.send_file(str(src), cua.SpaceSendFileOptions(target_directory="py-inbox"))
    assert sent.verified and len(sent.files) == 1
    assert (Path(fx["spaces_downloads"]) / "py-inbox" / "drop.txt").read_text() == "dropped"
    back = tmp_path / "back"
    back.mkdir()
    down = await space.download(guest, str(back))
    assert down.verified and (back / "note.txt").read_bytes() == b"from python"
    # A file big enough for the same-machine bulk lane (over 2 MiB) arrives
    # byte for byte, whichever way the Space's connection moves it.
    big = os.urandom(5 * 1024 * 1024 + 17)
    big_guest = str(Path(fx["spaces_guest_home"]) / "py" / "big.bin")
    assert (await space.write(big_guest, big)).bytes == len(big)
    down = await space.download(big_guest, str(back))
    assert down.verified and (back / "big.bin").read_bytes() == big

    tools = await space.list_tools(None)
    assert any(t.name == "get_screen_size" for t in tools)
    r = await space.call_tool("get_screen_size", "{}", None, None)
    assert not r.is_error and r.text == "1280x800"

    with pytest.raises(cua.CuaError.CapabilityMissing):
        await space.open_stream(cua.SpaceStreamOptions())

    await exercise_channel(space)

    ok = Approver(cua.TeleportDecision(include=None, acknowledge_sensitive=True))
    if teleport:
        # The Cua Spaces daemon: the manifest, and a session only through the
        # Keyvault (the daemon never delivers a caller-approved session).
        manifest = await space.teleport_manifest("firefox", None)
        assert any(i.is_sensitive for i in manifest.items)
        with pytest.raises(cua.CuaError.TeleportRefused):
            await space.teleport("firefox", None, Approver(None))
        with pytest.raises(cua.CuaError.TeleportRefused):
            await space.teleport("firefox", None, ok)
        # Named volumes of Cua Volume: make, list, refuse a duplicate, and
        # refuse a Space that mounts one that does not exist.
        assert await spaces.volumes() == []
        made = await spaces.create_volume("shared")
        assert made.name == "shared"
        with pytest.raises(cua.CuaError, match="already exists"):
            await spaces.create_volume("shared")
        assert [v.name for v in await spaces.volumes()] == ["shared"]
        with pytest.raises(cua.CuaError, match="does not exist"):
            await spaces.create(
                cua.SpaceCreateOptions(volumes=[cua.VolumeMount(name="ghost", path="/mnt/ghost")])
            )
        with pytest.raises(cua.CuaError.InvalidArgument, match="absolute"):
            await spaces.create(
                cua.SpaceCreateOptions(volumes=[cua.VolumeMount(name="shared", path="mnt/x")])
            )
        assert await spaces.delete_volume("shared", False) == 0
        assert await spaces.volumes() == []
    else:
        # Without Cua Spaces: missing, and it says where it ships.
        with pytest.raises(cua.CuaError.HostCapabilityMissing) as e:
            await space.teleport_manifest("firefox", None)
        assert "Cua Spaces" in str(e.value)
        with pytest.raises(cua.CuaError.HostCapabilityMissing):
            await space.teleport("firefox", None, ok)
        # Named volumes are Cua Volume's too.
        with pytest.raises(cua.CuaError.HostCapabilityMissing) as e:
            await spaces.volumes()
        assert "Cua Spaces" in str(e.value)
        with pytest.raises(cua.CuaError.HostCapabilityMissing):
            await spaces.create_volume("shared")
        with pytest.raises(cua.CuaError.HostCapabilityMissing):
            await spaces.create(
                cua.SpaceCreateOptions(volumes=[cua.VolumeMount(name="shared", path="/mnt/s")])
            )

    r = await spaces.call_tool_json("list_spaces", None)
    assert not r.is_error and info.id in r.text

    assert info.id in await spaces.delete(info.id)
    assert await spaces.list() == []


async def exercise_channel(space):
    """Which way calls go, and a TCP forward over the Space's own connection
    (cua-spacesd's tunnel), embedded and through the daemon's passthrough."""
    route = await space.route()
    # A Space added by its address: reached directly, by that address.
    assert route.startswith("direct http://127.0.0.1:"), route
    assert await space.spacesd().route() == route

    import socket
    import threading

    srv = socket.socket()
    srv.bind(("127.0.0.1", 0))
    srv.listen(1)
    guest_port = srv.getsockname()[1]

    def echo():
        conn, _ = srv.accept()
        with conn:
            try:
                while data := conn.recv(65536):
                    conn.sendall(data)
            except OSError:
                pass  # the test closed its side

    t = threading.Thread(target=echo, daemon=True)
    t.start()
    fwd = await space.forward(guest_port)
    try:
        assert fwd.guest_port() == guest_port
        host, port = fwd.local_addr().rsplit(":", 1)
        assert host == "127.0.0.1" and int(port) != guest_port
        assert fwd.url() == f"http://{fwd.local_addr()}"
        payload = os.urandom(256 * 1024)

        def round_trip():
            with socket.create_connection((host, int(port)), timeout=10) as c:
                sender = threading.Thread(target=c.sendall, args=(payload,), daemon=True)
                sender.start()
                got = b""
                while len(got) < len(payload) and (chunk := c.recv(65536)):
                    got += chunk
                sender.join(10)
                return got

        import asyncio

        assert await asyncio.get_running_loop().run_in_executor(None, round_trip) == payload
    finally:
        await fwd.close()
        srv.close()
    with pytest.raises(cua.CuaError):
        await space.forward(0)


def test_spaces_embedded(fixtures, tmp_path):
    c = cua.embedded(
        state_dir=str(tmp_path / "sbx"),
        spaces_home=str(tmp_path / "cua"),
        teleport_home=fixtures["teleport_host_home"],
    )
    run(exercise(c, fixtures, tmp_path))


@pytest.mark.skipif(os.name == "nt", reason="Unix socket")
def test_spaces_through_the_daemon(fixtures, cua_binary, tmp_path):
    daemon_exercise(fixtures, cua_binary, tmp_path, teleport=False)


@pytest.mark.skipif(os.name == "nt", reason="Unix socket")
def test_spaces_through_the_cua_spaces_daemon(fixtures, spaces_cli_binary, tmp_path):
    daemon_exercise(fixtures, spaces_cli_binary, tmp_path, teleport=True)


def daemon_exercise(fixtures, cua_binary, tmp_path, teleport: bool):
    home = Path(tempfile.mkdtemp(prefix="cua-pys-", dir="/tmp"))
    sock = home / "cua.sock"
    env = dict(
        os.environ,
        CUA_HOME=str(home),
        HOME=str(home),
        CUA_DAEMON_NO_RELAY="1",
        CUA_SPACES_TELEPORT_HOME=fixtures["teleport_host_home"],
        CUA_SPACES_AGENT_CREDENTIALS_HOME="none",
    )
    daemon = subprocess.Popen(
        [
            str(cua_binary),
            "daemon",
            "start",
            "--foreground",
            "--socket",
            str(sock),
            "--state-dir",
            str(home / "sbx"),
        ],
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        wait_for(sock.exists, "daemon socket", timeout=15)
        c = cua.connect(str(sock))
        run(exercise(c, fixtures, tmp_path, teleport=teleport))
        # The daemon's registry, not this process's.
        assert (home / "spaces.json").exists()
        run(c.shutdown_daemon())
        daemon.wait(timeout=15)
    finally:
        if daemon.poll() is None:
            daemon.terminate()
            daemon.wait(timeout=15)

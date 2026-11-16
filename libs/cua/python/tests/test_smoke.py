"""Smoke tests of the generated binding against loopback fixtures, in the
embedded topology and against a `cua daemon` started from the CLI."""

from __future__ import annotations

import asyncio
import os
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

import pytest

import cua


def run(coro):
    return asyncio.run(asyncio.wait_for(coro, timeout=60))


def env_supports_run_as(caps):
    return any(
        f.name == "process.run_as" and f.attributes.get("mode") in ("root", "capabilities")
        for f in caps.features
    )


def command(program, *args, stdin=False):
    return cua.SpacesdCommand(program=program, args=list(args), stdin=stdin)


class Sink(cua.FrameSink, cua.AudioSink):
    def __init__(self):
        self.frames = []
        self.events = []
        self.audio = []
        self.lock = threading.Lock()

    def on_frame(self, frame):
        with self.lock:
            self.frames.append(frame)

    def on_event(self, event):
        with self.lock:
            self.events.append(event)

    def on_audio(self, packet):
        with self.lock:
            self.audio.append(packet)


def wait_for(cond, what, timeout=5.0):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if cond():
            return
        time.sleep(0.02)
    raise AssertionError(f"timed out waiting for {what}")


async def file_access(env) -> None:
    """Acting as another guest user, and extended-attribute metadata
    (the fixture guest knows alice and bob, and refuses `locked`)."""
    opts = cua.UploadOptions(
        user="alice", metadata={"origin": "py"}, xattr_namespace="user.e2b."
    )
    await env.upload("/tmp/py/as.bin", b"as alice", opts)
    st = await env.stat("/tmp/py/as.bin", user="alice", xattr_namespace="user.e2b.")
    assert st.owner == "alice"
    assert st.metadata == {"origin": "py"}
    # The default namespace does not see the e2b one.
    assert (await env.stat("/tmp/py/as.bin")).metadata == {}
    rows = await env.list_dir("/tmp/py", 1, user="alice", xattr_namespace="user.e2b.")
    assert {r.name: r.metadata.get("origin") for r in rows}["as.bin"] == "py"
    assert await env.download("/tmp/py/as.bin", user="alice") == b"as alice"
    moved = await env.move_path("/tmp/py/as.bin", "/tmp/py/as2.bin", create_parents=True, user="alice")
    assert moved.path == "/tmp/py/as2.bin"
    await env.remove("/tmp/py/as2.bin", False, user="alice")
    made = await env.make_dir("/tmp/py/dir", user="bob")
    assert made.owner == "bob"
    # `user` also rides on raw requests.
    raw = await env.call_json(
        "FilesystemService/Stat", '{"path": "/tmp/py/blob"}', user="bob"
    )
    assert '"entry"' in raw
    with pytest.raises(cua.CuaError.Env, match="no-such"):
        await env.stat("/tmp/py/blob", user="no-such")
    with pytest.raises(cua.CuaError.PermissionDenied):
        await env.stat("/tmp/py/blob", user="locked")
    with pytest.raises(cua.CuaError.InvalidArgument):
        await env.call_json("SystemService/Health", "{}", user="alice")


async def exercise(c: cua.Cua, fx: dict) -> None:
    sbx = c.sandboxes()
    sb = await sbx.connect_url(fx["env_url"], fx["env_token"], "py-direct")
    assert sb.name() == "py-direct"
    assert sb.location() == "direct"
    env = await sb.spacesd(5000)

    caps = await env.capabilities()
    assert caps.version
    # How the guest agent runs commands and file operations as another user:
    # itself, with no sudo (`root` or `capabilities`), through sudo (older
    # images) or not at all (`none`).
    run_as = {f.name: f for f in caps.features}["process.run_as"]
    assert run_as.supported and run_as.attributes["mode"] == "capabilities"
    assert env_supports_run_as(caps)
    out = await env.run(
        cua.SpacesdCommand(program="echo", args=["as alice"], user="alice")
    )
    assert out.exit.success and out.stdout == b"as alice\n"
    out = await env.run(command("echo", "hi"))
    assert out.exit.success and out.stdout == b"hi\n"
    out = await env.sh("fail 4", None)
    assert out.exit.code == 4

    proc = await env.spawn(command("cat", stdin=True))
    await proc.write_stdin(b"xyz")
    await proc.close_stdin()
    assert (await proc.wait()).stdout == b"xyz"

    blob = bytes(range(256)) * 1000
    up = await env.upload("/tmp/py/blob", blob, None)
    assert up.size == len(blob)
    assert await env.download("/tmp/py/blob") == blob

    await file_access(env)

    await env.set_clipboard("from python")
    assert await env.get_clipboard() == "from python"
    await env.click(1.0, 2.0)
    assert (await env.call_json("SystemService/Health", "{}")).startswith("{")

    with pytest.raises(cua.CuaError.NotFound):
        await env.download("/does/not/exist")
    with pytest.raises(cua.CuaError.InvalidArgument):
        await env.call_json("ProcessService/StartProcess", "{}")

    sink = Sink()
    session = await env.open_media_with_audio(cua.MediaOpenOptions(audio=True), sink, sink)
    assert session.codec() == "h264"
    wait_for(lambda: len(sink.frames) >= 2 and sink.audio, "frames")
    assert sink.frames[0].keyframe and sink.frames[0].sequence == 7
    assert sink.audio[0].frame_samples == 960
    assert [e.kind for e in sink.events[:2]] == ["hello", "session_opened"]
    await session.close()

    bad = await sbx.connect_url(fx["env_url"], "wrong", "py-bad")
    with pytest.raises(cua.CuaError.Unauthenticated):
        await bad.spacesd(3000)
    # One machine, one ref (`direct:<host:port>`): the latest connection to
    # an address is the one the ref reaches. Reconnect with the right token.
    assert bad.id() == sb.id() and sb.id().startswith("direct:127.0.0.1:")
    sb = await sbx.connect_url(fx["env_url"], fx["env_token"], "py-direct")

    # A machine cua only connects to has no shape of its own to change.
    with pytest.raises(cua.CuaError.Unsupported):
        await sb.resize(2, 2048, None)

    listing = await sbx.list_with_warnings(None)
    rows = {s.name: s for s in listing.sandboxes}
    assert rows["py-direct"].location == "direct"
    assert listing.warnings == []  # Cua Cloud has ended: no cloud, silently
    # Local only: a direct URL is not local.
    assert "py-direct" not in [s.name for s in await sbx.list("local")]
    await sb.delete()
    with pytest.raises(cua.CuaError.NotFound):
        await sbx.get("nope")

    report = await c.local().doctor()
    assert any(ch.name == "qemu" for ch in report.checks)
    assert '"backends"' in report.report_json
    with pytest.raises(cua.CuaError.InvalidArgument):
        await c.local().setup(["nope"], True)


def test_embedded(fixtures, tmp_path):
    c = cua.embedded(state_dir=str(tmp_path / "sbx"))
    assert c.mode() == cua.CuaMode.EMBEDDED
    run(exercise(c, fixtures))


def _assert_cloud_ended(info):
    """Cua Cloud has ended (CUA-1854): the one message, as `Unsupported`."""
    assert isinstance(info.value, cua.CuaError.Unsupported)
    assert "Cua Cloud has ended" in str(info.value)


def test_cloud_creates_fail_with_the_cloud_ended_error(fixtures, tmp_path):
    c = cua.embedded(state_dir=str(tmp_path / "sbx"))

    async def body():
        sbx = c.sandboxes()
        for opts in (
            cua.SandboxCreateOptions(on="cloud", image="img:test", kind="vm"),
            cua.SandboxCreateOptions(image="pool:a-pool"),
        ):
            with pytest.raises(cua.CuaError.Unsupported) as info:
                await sbx.create(opts)
            _assert_cloud_ended(info)
        # Nothing was created, and cloud is not a listed location.
        assert (await sbx.list_with_warnings("cloud")).sandboxes == []
        assert "cloud" not in cua.sandbox_locations()

    run(body())


@pytest.mark.skipif(os.name == "nt", reason="Unix socket")
def test_daemon_topology(fixtures, cua_binary):
    # Short path: macOS limits Unix socket paths to 104 bytes.
    home = Path(tempfile.mkdtemp(prefix="cua-py-", dir="/tmp"))
    sock = home / "cua.sock"
    env = dict(os.environ, CUA_HOME=str(home), HOME=str(home))
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
        assert c.mode() == cua.CuaMode.DAEMON
        info = run(c.info())
        assert info.daemon_pid == daemon.pid
        run(exercise(c, fixtures))
        run(c.shutdown_daemon())
        daemon.wait(timeout=15)
    finally:
        if daemon.poll() is None:
            daemon.terminate()
            daemon.wait(timeout=15)


def test_cua_console_command_runs_the_bundled_cli():
    """The wheel ships the Rust `cua` CLI behind the `cua` console script."""
    from cua import _cli

    if not _cli.binary().exists():
        pytest.skip("wheel built with --no-cli")
    out = subprocess.run(
        [sys.executable, "-m", "cua._cli", "--version"],
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert out.returncode == 0, out.stderr
    assert out.stdout.startswith("cua ")


def test_headless_create_and_pool_options():
    # Hermetic: records only. `headless` (no desktop, only cua-spacesd) on
    # creates and pools, and a pool's own `command`.
    assert cua.SandboxCreateOptions(on="local").headless is False
    assert cua.SandboxCreateOptions(on="local", headless=True).headless is True
    pool = cua.PoolCreateOptions(command=["sleep", "1"], headless=True)
    assert pool.command == ["sleep", "1"]
    assert pool.headless is True
    assert cua.PoolCreateOptions().command is None
    assert cua.PoolCreateOptions().headless is False


def test_spacesd_option_on_creates():
    # Hermetic: records only. `spacesd` (None: a static cua-spacesd is put
    # into an OCI image that has none, on a container or microVM, so any image
    # is a full Space; False runs the image as it is).
    assert cua.SandboxCreateOptions(on="local").spacesd is None
    assert cua.SandboxCreateOptions(on="local", spacesd=False).spacesd is False
    assert cua.SpaceCreateOptions().spacesd is None
    assert cua.SpaceCreateOptions(spacesd=False).spacesd is False

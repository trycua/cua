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


async def exercise(c: cua.Cua, fx: dict) -> None:
    sbx = c.sandboxes()
    sb = await sbx.connect_url(fx["env_url"], fx["env_token"], "py-direct")
    assert sb.name() == "py-direct"
    assert sb.location() == "direct"
    env = await sb.spacesd(5000)

    caps = await env.capabilities()
    assert caps.version
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

    listing = await sbx.list_with_warnings(None)
    rows = {s.name: s for s in listing.sandboxes}
    assert rows["py-direct"].location == "direct"
    assert listing.warnings == []  # no Fleet credentials: no cloud, silently
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
    c = cua.embedded(state_dir=str(tmp_path / "sbx"), fleet_from_env=False)
    assert c.mode() == cua.CuaMode.EMBEDDED
    run(exercise(c, fixtures))


def test_embedded_fleet_through_fake_api(fixtures, tmp_path):
    c = cua.embedded(
        state_dir=str(tmp_path / "sbx"),
        fleet_from_env=False,
        fleet=cua.FleetSettings(base_url=fixtures["fleet_base_url"], token=fixtures["fleet_token"]),
    )

    async def body():
        fleet = c.fleet()
        pool = await fleet.apply_pool(
            cua.FleetPoolSpec(name="cua-e2e-py", image="img:test", runtime="gvisor")
        )
        assert pool.name == "cua-e2e-py"
        sb = await c.sandboxes().create(
            cua.SandboxCreateOptions(
                on="cloud", pool="cua-e2e-py", name="cua-e2e-py-claim"
            )
        )
        assert sb.location() == "cloud"
        # The claim's env service is the fixture's cua-spacesd (through the
        # fake gateway), which speaks gRPC, not plain HTTP.
        out = await (await sb.spacesd(5_000)).sh("echo claim", None)
        assert bytes(out.stdout) == b"claim\n"
        await sb.delete()
        await fleet.delete_pool("cua-e2e-py")

    run(body())


def test_unconfigured_fleet_is_a_typed_error(tmp_path):
    c = cua.embedded(state_dir=str(tmp_path), fleet_from_env=False)
    with pytest.raises(cua.CuaError.ProviderNotConfigured):
        c.fleet()


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

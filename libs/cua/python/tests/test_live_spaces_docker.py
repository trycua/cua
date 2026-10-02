"""Live Spaces through a real `cua daemon` and the Python binding.

Gated: CUA_E2E_SPACES_DOCKER=1 and a Docker engine (Colima) with `runsc`.

    docker run the linux image (gVisor, --memory=4g)  ->  cua daemon (temp HOME)
      ->  cua.connect()  ->  spaces.add  ->  bash, send_file, stream_session
      (frames arrive, keyframe first)  ->  remove

The container is named `cua-e2e-spaces-py-*` and removed in `finally`; the
daemon runs with a temp HOME/CUA_HOME and is shut down. Nothing touches the
host's apps, profiles or keychain.
"""

from __future__ import annotations

import asyncio
import hashlib
import os
import secrets
import shutil
import subprocess
import tempfile
import threading
import time
import urllib.request
from pathlib import Path

import pytest

import cua

IMAGE = os.environ.get("CUA_E2E_SPACES_IMAGE", "cua-e2e-local/linux:docker-local-arm64")
RUNTIME = os.environ.get("CUA_E2E_SPACES_RUNTIME", "runsc")

pytestmark = pytest.mark.skipif(
    os.environ.get("CUA_E2E_SPACES_DOCKER") != "1",
    reason="set CUA_E2E_SPACES_DOCKER=1 for the live docker Spaces e2e",
)


def wait(what, cond, timeout):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if cond():
            return
        time.sleep(0.5)
    raise AssertionError(f"timed out waiting for {what}")


class Frames(cua.FrameSink):
    def __init__(self):
        self.frames = []
        self.events = []
        self.lock = threading.Lock()

    def on_frame(self, frame):
        with self.lock:
            if len(self.frames) < 200:  # bounded
                self.frames.append((frame.sequence, frame.keyframe, frame.codec, len(frame.data)))

    def on_event(self, event):
        with self.lock:
            if len(self.events) < 200:
                self.events.append(event.kind)


def test_spaces_add_bash_send_file_stream_remove_through_the_daemon(cua_binary, tmp_path):
    docker = shutil.which("docker")
    assert docker, "docker not on PATH"
    name = f"cua-e2e-spaces-py-{secrets.token_hex(3)}"
    token = secrets.token_hex(16)
    home = Path(tempfile.mkdtemp(prefix="cua-e2e-", dir="/tmp"))
    sock = home / "cua.sock"
    env_file = home / "env"
    env_file.write_text(f"CUA_ENV_TOKEN={token}\n")
    env_file.chmod(0o600)
    daemon = None
    try:
        subprocess.run(
            [
                docker,
                "run",
                "-d",
                "--name",
                name,
                f"--runtime={RUNTIME}",
                "--memory=4g",
                "--memory-swap=4g",
                "--shm-size=512m",
                "--env-file",
                str(env_file),
                "-p",
                "127.0.0.1::3211",
                IMAGE,
            ],
            check=True,
            capture_output=True,
            timeout=120,
        )
        port = None

        def healthy():
            nonlocal port
            st = subprocess.run(
                [docker, "inspect", "-f", "{{.State.Health.Status}}", name],
                capture_output=True,
                text=True,
                timeout=30,
            ).stdout.strip()
            if st != "healthy":
                return False
            out = subprocess.run(
                [docker, "port", name, "3211/tcp"], capture_output=True, text=True, timeout=30
            ).stdout.splitlines()[0]
            port = int(out.rsplit(":", 1)[1])
            try:
                with urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=3) as r:
                    return r.status in (200, 204)
            except Exception:
                return False

        wait("container health", healthy, 180)
        url = f"http://127.0.0.1:{port}"

        env = dict(
            os.environ,
            HOME=str(home),
            CUA_HOME=str(home),
            CUA_SPACES_TELEPORT_HOME=str(home / "host-home"),
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
            stderr=open(home / "daemon.log", "w"),
        )
        wait("daemon socket", sock.exists, 20)
        c = cua.connect(str(sock))

        async def body():
            info = await c.info()
            assert info.mode == cua.CuaMode.DAEMON
            spaces = c.spaces()
            space_info = await spaces.add(url, token, "e2e-docker")
            assert "desktop_stream" in space_info.features, space_info.features
            space = await spaces.space(space_info.id)

            out = await space.bash("uname -s; id -un", 60_000)
            assert out.exit_code == 0 and out.stdout.startswith("Linux"), out

            payload = os.urandom(3 * 1024 * 1024 + 7)
            src = tmp_path / "payload.bin"
            src.write_bytes(payload)
            sent = await space.send_file(
                str(src), cua.SpaceSendFileOptions(target_directory="cua-e2e")
            )
            assert sent.verified and len(sent.files) == 1, sent
            guest_sha = await space.bash(f"sha256sum '{sent.files[0].path}'", 60_000)
            assert guest_sha.stdout.split()[0] == hashlib.sha256(payload).hexdigest()

            sink = Frames()
            session = await space.stream_session(
                cua.SpaceStreamOptions(max_fps=10, max_dimension=1280), sink, None
            )
            try:

                def count():
                    with sink.lock:
                        return len(sink.frames)

                deadline = time.monotonic() + 30
                while time.monotonic() < deadline and count() < 1:
                    await asyncio.sleep(0.2)
                assert count() >= 1, sink.events
                # A still desktop sends no damage frames; ask for keyframes
                # (the control path) and also draw on the guest's display.
                await space.bash(
                    "DISPLAY=:1 xsetroot -solid '#336699' 2>/dev/null || "
                    "DISPLAY=:0 xsetroot -solid '#336699' 2>/dev/null || true",
                    30_000,
                )
                for _ in range(3):
                    session.request_keyframe()
                    await asyncio.sleep(1.0)
                deadline = time.monotonic() + 20
                while time.monotonic() < deadline and count() < 3:
                    await asyncio.sleep(0.2)
                with sink.lock:
                    frames = list(sink.frames)
                assert len(frames) >= 3, (frames, sink.events)
                assert frames[0][1], "first delivered frame is a keyframe"
                assert frames[0][2] == "h264"
                assert session.stats().frames >= 3
            finally:
                final = await session.close()
            print(f"frames={final.frames} keyframes={final.keyframes} codec={frames[0][2]}")

            await spaces.remove(space_info.id)
            assert await spaces.list() == []
            await c.shutdown_daemon()

        asyncio.run(asyncio.wait_for(body(), timeout=300))
        daemon.wait(timeout=20)
    finally:
        if daemon is not None and daemon.poll() is None:
            daemon.terminate()
            daemon.wait(timeout=20)
        subprocess.run([docker, "rm", "-f", name], capture_output=True, timeout=60)
        shutil.rmtree(home, ignore_errors=True)

"""Any OCI image is a full Space through the Python binding. Gated:
CUA_E2E_OCI_ANY=1 and a local container engine (or the built-in runtime on a
Mac). Creates `alpine:3.20` and `python:3.12-slim` Spaces (neither carries
cua-spacesd; the runtime injects a static one), runs a command, writes and
reads a file, reaches a server the Space starts through a port forward, and
deletes each Space in `finally`."""

from __future__ import annotations

import asyncio
import os
import secrets
import urllib.request

import pytest

import cua

KIND = os.environ.get("CUA_E2E_OCI_ANY_KIND", "container")

pytestmark = pytest.mark.skipif(
    os.environ.get("CUA_E2E_OCI_ANY") != "1", reason="set CUA_E2E_OCI_ANY=1 for the live OCI e2e"
)

CASES = [
    (
        "alpine:3.20",
        "Alpine",
        r"""sh -c 'while true; do printf "HTTP/1.0 200 OK\r\nContent-Length: 7\r\n\r\nAlpine\n" | nc -l -p 8000 >/dev/null; done'""",
    ),
    ("python:3.12-slim", "Debian", "python -m http.server 8000 --directory /etc"),
]


@pytest.mark.parametrize("image,marker,server", CASES)
def test_any_oci_image_is_a_full_space(tmp_path, image, marker, server):
    name = f"cua-e2e-py-oci-{secrets.token_hex(3)}"

    async def body():
        c = cua.embedded(state_dir=str(tmp_path), spaces_home=str(tmp_path / "spaces"))
        spaces = c.spaces()
        made = await spaces.create(
            cua.SpaceCreateOptions(
                image=image, on="local", kind=KIND, name=name, cpus=1, memory_mb=1024, timeout_ms=600_000
            )
        )
        assert made.space is not None
        try:
            space = await spaces.space(made.space.id)
            out = await space.bash("cat /etc/os-release", None)
            assert out.exit_code == 0 and marker in out.stdout, out

            report = await space.write("/tmp/note.txt", b"from python")
            assert report.bytes == 11
            back = await space.bash("cat /tmp/note.txt", None)
            assert back.stdout == "from python"

            started = await space.bash(f"nohup {server} >/tmp/server.log 2>&1 & sleep 1; echo up", None)
            assert started.exit_code == 0, started
            fwd = await space.forward(8000, None)
            addr = fwd.local_addr()
            text = await asyncio.to_thread(
                lambda: urllib.request.urlopen(f"http://{addr}/os-release", timeout=10).read().decode()
            )
            assert marker in text
            await fwd.close()
        finally:
            await spaces.delete(made.space.id)

    asyncio.run(body())


def test_spacesd_false_runs_the_image_as_it_is(tmp_path):
    name = f"cua-e2e-py-oci-bare-{secrets.token_hex(3)}"

    async def body():
        c = cua.embedded(state_dir=str(tmp_path))
        sb = await c.sandboxes().create(
            cua.SandboxCreateOptions(
                on="local",
                kind=KIND,
                image="alpine:3.20",
                name=name,
                cpus=1,
                memory_mb=512,
                command=["sleep", "300"],
                spacesd=False,
                ready_timeout_ms=300_000,
            )
        )
        try:
            assert sb.lacks_spacesd(), "an image run as it is has no cua-spacesd"
        finally:
            await sb.delete()

    asyncio.run(body())

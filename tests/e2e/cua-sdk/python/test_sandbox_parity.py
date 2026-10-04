"""sandbox-parity: sidecars, env, private registry secrets and image layers.

Lanes:

* hermetic: a cloud create fails at once (Cua Cloud has closed); a local
  core create refuses layers on a VM image.
* container: ``python:3.12-slim`` with a ``redis:7-alpine`` sidecar named
  ``db`` and a ``busybox`` sidecar named ``relay``, through a private
  ``cua daemon`` on runc. The sandbox reaches ``db:6379`` by name and the
  relay reaches the sandbox at ``main``; refused on gVisor without an
  explicit runc.
"""

from __future__ import annotations

import base64

import e2e
import pytest

import cua

ROOTFS_IMAGE = "registry.example/mcp:docker-e2e"
GREETING = "hi-from-env"

# The sandbox's server: PINGs redis at db:6379 (by name), then serves
# "<GREETING> <reply>" on 8000. One line, so the script travels
# base64-encoded.
_SERVER = """
import os, socket, time, http.server
def ping(host):
    for _ in range(600):
        try:
            s = socket.create_connection((host, 6379), 2)
            s.sendall(b"PING\\r\\n")
            r = s.recv(64)
            s.close()
            return r.decode().strip()
        except OSError:
            time.sleep(1)
    return "unreachable"
reply = ping(os.environ.get("REDIS_HOST", "db")) if os.environ.get("REDIS_HOST") else "none"
body = (os.environ.get("GREETING", "unset") + " " + reply).encode()
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *a):
        pass
http.server.HTTPServer(("0.0.0.0", 8000), H).serve_forever()
"""
SERVER = [
    "python3",
    "-c",
    "import base64;exec(base64.b64decode('%s'))" % base64.b64encode(_SERVER.encode()).decode(),
]
# A sidecar that fetches the sandbox's page from http://main:8000 (the
# sandbox, by name) and serves it on 8081.
RELAY = [
    "sh",
    "-c",
    "mkdir -p /www; until wget -q -O /www/index.html http://main:8000/; do sleep 2; done; "
    "exec httpd -f -p 8081 -h /www",
]


def _redis(name: str | None = None) -> "cua.Container":
    return cua.Container(image="redis:7-alpine", command=None, env={}, ports=[6379], name=name)


def _relay() -> "cua.Container":
    return cua.Container(image="busybox:1.36", command=RELAY, env={}, ports=[8081], name="relay")


def _opts(on: str, image: str, what: str, **kw) -> "cua.SandboxCreateOptions":
    kw.setdefault("cpus", 1)
    kw.setdefault("memory_mb", 512)
    return cua.SandboxCreateOptions(on=on, image=image, name=e2e.name(what), **kw)


def _group(on: str, image: str, what: str, **kw) -> "cua.SandboxCreateOptions":
    """The sandbox + db + relay group: the relay's page proves both ways."""
    return _opts(
        on,
        image,
        what,
        command=SERVER,
        env={"GREETING": GREETING, "REDIS_HOST": "db"},
        services={"web": 8000, "db": 6379, "relay": 8081},
        wait_for=[cua.ReadinessProbe(port=0, service="relay", http_path="/")],
        sidecars=[_redis("db"), _relay()],
        **kw,
    )


async def _expect_group(sb) -> None:
    web = await sb.service("web").request("GET", "/", None, 60_000, None)
    assert bytes(web.body).strip() == f"{GREETING} +PONG".encode(), web.body
    relay = await sb.service("relay").request("GET", "/", None, 60_000, None)
    assert bytes(relay.body).strip() == f"{GREETING} +PONG".encode(), relay.body
    assert sb.info().services.get("db") == 6379


@pytest.mark.e2e("sandbox-parity", "hermetic")
def test_parity_cloud_closed_and_local_refuses_vm_layers(tmp_path):
    async def body():
        local = cua.embedded(state_dir=str(tmp_path / "local"), fleet_from_env=False)
        with pytest.raises(cua.CuaError.Fleet, match="Cua Cloud has closed"):
            await local.sandboxes().create(
                _opts("cloud", ROOTFS_IMAGE, "parity", sidecars=[_redis("db")])
            )
        # Locally, layers build into the container engine (the container
        # lane runs that); a VM image takes none, refused before anything runs.
        with pytest.raises(cua.CuaError.Unsupported):
            await local.sandboxes().create(
                _opts(
                    "local",
                    "vm:ghcr.io/trycua/linux:24.04-disk",
                    "parity-local",
                    build=cua.ImageBuild(
                        layers=[cua.ImageLayer.RUN(command="true")],
                        env={},
                        ports=[],
                        files=[],
                        timeout_ms=None,
                    ),
                )
            )

    e2e.run_async(body(), 120)


@pytest.mark.e2e("sandbox-parity", "container")
def test_parity_sidecars_by_name_through_the_daemon():
    for image in ("python:3.12-slim", "redis:7-alpine", "busybox:1.36"):
        if not e2e.docker_image_exists(image):
            e2e.docker("pull", image)

    async def body():
        with e2e.cua_daemon() as d:
            c = d.client()
            o = _group("local", "python:3.12-slim", "parity-sc", ready_timeout_ms=240_000)
            # Where gVisor would run, the group is refused unless runc is
            # chosen explicitly (no silent downgrade).
            if "runsc" in e2e.docker("info", "--format", "{{json .Runtimes}}").stdout:
                with pytest.raises(cua.CuaError.Unsupported, match="runtime='runc'"):
                    await c.sandboxes().create(o)
            o.runtime = "runc"
            sb = await c.sandboxes().create(o)
            name = sb.info().name
            try:
                await _expect_group(sb)
            finally:
                await sb.delete()
            left = e2e.docker("ps", "-a", "--filter", f"name={name}", "--format", "{{.Names}}")
            assert not left.stdout.strip(), left.stdout

    e2e.run_async(body(), 600)

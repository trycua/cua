"""Shared helpers for the cua SDK e2e suite (Python).

Conventions (the TypeScript and Rust halves mirror them):

* Every resource is named ``cua-e2e-<run>-<what>`` (``CUA_E2E_RUN``, random
  by default) and is deleted in ``finally``.
* Lanes are opt-in through env vars (see ``scenarios.json``); a test that
  cannot run in the current environment *skips with a reason*, and
  ``run.py --strict`` turns an unexpected skip into a failure in CI.
* Memory caps: desktop containers 2 GiB, plain containers 512 MiB, VMs
  <= 4 GiB, one VM at a time. Every poll loop has an iteration bound.
* Results go to ``$CUA_E2E_RESULTS/py.jsonl`` (one line per test).
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import os
import platform
import secrets
import shutil
import socket
import subprocess
import time
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Iterator, Optional

import cua

HERE = Path(__file__).resolve().parent
SUITE = HERE.parent
REPO = SUITE.parents[2]
CUA_ROOT = REPO / "libs" / "cua"

RUN = os.environ.get("CUA_E2E_RUN") or secrets.token_hex(3)
os.environ["CUA_E2E_RUN"] = RUN

HOST_ARCH = "arm64" if platform.machine().lower() in ("arm64", "aarch64") else "amd64"

# The legacy public Fleet image the guides use (computer-server on 8000, no
# spacesd). Pinned by digest exactly as in the guides.
LEGACY_FLEET_IMAGE = os.environ.get(
    "CUA_E2E_FLEET_IMAGE",
    "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04"
    "@sha256:82702ebdd32d1f8fc05f2ea409a7c67d0ba9f8f8e4e9f1a89ce40989d5f4475d",
)
# Same image, gVisor rootfs tag (cheap and fast on Fleet).
LEGACY_FLEET_ROOTFS = os.environ.get(
    "CUA_E2E_FLEET_ROOTFS",
    "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:docker-main-809e3f81",
)


def desktop_image() -> str:
    return os.environ.get("CUA_E2E_DESKTOP_IMAGE", f"cua-e2e-local/linux:docker-local-{HOST_ARCH}")


def plain_image(name: str) -> str:
    env = {
        "ubuntu-server": "CUA_E2E_PLAIN_SERVER_IMAGE",
        "ubuntu-xfce-vnc": "CUA_E2E_PLAIN_VNC_IMAGE",
    }[name]
    return os.environ.get(env, f"cua-e2e-local/{name}:docker-local-{HOST_ARCH}")


def disk_path(image: str, arch: str = HOST_ARCH) -> Path:
    return Path(
        os.environ.get(
            f"CUA_E2E_DISK_{image.upper().replace('-', '_')}",
            str(Path.home() / ".cache" / "cua-images-e2e" / image / arch / "disk.img"),
        )
    )


def name(what: str) -> str:
    """``cua-e2e-<run>-<what>-py`` (DNS label, <= 63 chars)."""
    return f"cua-e2e-{RUN}-{what}-py"[:63].rstrip("-")


# ---------------------------------------------------------------- lanes


# Fleet pools have no env/secret field yet, so the spacesd token reaches a
# gVisor pod through an entrypoint override (the image reads
# /etc/cua/env-token). KubeVirt containerDisk guests have no such hook.
KUBEVIRT_ENV_SKIP = (
    "KubeVirt spacesd lane needs a per-claim secret field (cloud PR): no way to "
    "deliver the env token to a containerDisk guest yet"
)


def env_token_command(token: str) -> list[str]:
    """Pod command for a gVisor pool of the spacesd image that installs
    ``token`` as the driver's token before the normal entrypoint."""
    return [
        "/bin/sh",
        "-c",
        f"mkdir -p /etc/cua && printf %s {token} >/etc/cua/env-token && "
        "exec /opt/cua/desktop/entrypoint.sh",
    ]


def lane_enabled(lane: str) -> Optional[str]:
    """``None`` when ``lane`` can run here, else the skip reason."""
    flag = lambda v: os.environ.get(v) == "1"  # noqa: E731
    if lane == "hermetic":
        return None if fixtures_binary() else "cua-test-fixtures is not built"
    if lane == "container":
        if not flag("CUA_E2E_CONTAINER"):
            return "set CUA_E2E_CONTAINER=1 for the container lane"
        return None if shutil.which("docker") else "docker is not on PATH"
    if lane == "qemu":
        return None if flag("CUA_E2E_QEMU") else "set CUA_E2E_QEMU=1 for the QEMU lane"
    if lane == "lume":
        return None if flag("CUA_E2E_LUME") else "set CUA_E2E_LUME=1 for the Lume lane"
    if lane == "fleet":
        if not flag("CUA_E2E_FLEET"):
            return "set CUA_E2E_FLEET=1 for live Fleet"
        if not (os.environ.get("FLEETS_TOKEN") or os.environ.get("CUA_CLIENT_ID")):
            return "no Fleet credentials (CUA_CLIENT_ID/SECRET or FLEETS_TOKEN)"
        return None
    if lane == "fleet-env":
        why = lane_enabled("fleet")
        if why:
            return why
        if not os.environ.get("CUA_E2E_FLEET_ENV_IMAGE"):
            return (
                "CUA_E2E_FLEET_ENV_IMAGE is unset: no linux (spacesd) image "
                "in a registry Fleet can pull"
            )
        return None
    if lane == "docs":
        return None
    if lane == "contrib":
        why = _contrib_missing()
        # The contrib CI lane sets CUA_CONTRIB_REQUIRE=1: there a missing
        # prerequisite is a failure, not a skip.
        if why and os.environ.get("CUA_CONTRIB_REQUIRE") == "1":
            raise RuntimeError(f"contrib lane required: {why}")
        return why
    if lane == "cua-sandbox":
        return (
            None
            if flag("CUA_E2E_CUA_SANDBOX")
            else "set CUA_E2E_CUA_SANDBOX=1 (after the cua-sandbox wrapper merge)"
        )
    raise ValueError(f"unknown lane {lane}")


def _contrib_missing() -> Optional[str]:
    if not os.environ.get("CUA_CONTRIB_FIXTURES"):
        return "set CUA_CONTRIB_FIXTURES to the cua-contrib-fixtures binary for the contrib lane"
    try:
        import cua._native as native

        built = native.contrib_providers_built()
    except Exception as error:  # noqa: BLE001 - binding missing or too old
        return f"the cua binding cannot list contrib providers ({error})"
    if "daytona" not in built:
        return "the cua binding was built without contrib providers (--features contrib)"
    return None


def fixtures_binary() -> Optional[Path]:
    return _binary("CUA_TEST_FIXTURES", "cua-test-fixtures")


def cua_cli() -> Optional[Path]:
    return _binary("CUA_CLI", "cua")


def _binary(env: str, exe: str) -> Optional[Path]:
    explicit = os.environ.get(env)
    if explicit:
        return Path(explicit)
    for profile in ("debug", "release"):
        p = CUA_ROOT / "target" / profile / exe
        if p.exists():
            return p
    return None


# ---------------------------------------------------------------- misc


def run_async(coro, timeout: float = 600):
    return asyncio.run(asyncio.wait_for(coro, timeout=timeout))


async def poll(what: str, fn, *, attempts: int = 60, delay: float = 1.0, retry=(Exception,)):
    """Awaits ``fn()`` until it returns a truthy value (bounded). Exceptions
    not in ``retry`` propagate at once."""
    last: Any = None
    for _ in range(attempts):
        try:
            last = await fn()
            if last:
                return last
        except retry as e:  # noqa: BLE001 - surfaced on timeout
            last = e
        await asyncio.sleep(delay)
    raise AssertionError(f"timed out waiting for {what}; last={last!r}")


def http_get(url: str, *, headers: Optional[dict] = None, timeout: float = 10) -> tuple[int, bytes]:
    req = urllib.request.Request(url, headers=headers or {})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.status, r.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()


def http_post_json(
    url: str, body: dict, *, headers: Optional[dict] = None, timeout: float = 15
) -> tuple[int, dict, bytes]:
    h = {"content-type": "application/json", "accept": "application/json, text/event-stream"}
    h.update(headers or {})
    req = urllib.request.Request(url, data=json.dumps(body).encode(), headers=h, method="POST")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.status, dict(r.headers), r.read()
    except urllib.error.HTTPError as e:
        return e.code, dict(e.headers), e.read()


def read_banner(addr: str, n: int = 12, timeout: float = 10) -> bytes:
    host, port = addr.rsplit(":", 1)
    with socket.create_connection((host, int(port)), timeout=timeout) as s:
        s.settimeout(timeout)
        data = b""
        for _ in range(16):  # bounded
            chunk = s.recv(n - len(data))
            if not chunk:
                break
            data += chunk
            if len(data) >= n:
                break
        return data


def is_png(b: bytes) -> bool:
    return b[:8] == b"\x89PNG\r\n\x1a\n"


def png_size(b: bytes) -> tuple[int, int]:
    return int.from_bytes(b[16:20], "big"), int.from_bytes(b[20:24], "big")


def docker(*args: str, check: bool = True, timeout: float = 120) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["docker", *args], capture_output=True, text=True, check=check, timeout=timeout
    )


def docker_has_runsc() -> bool:
    out = docker("info", "--format", "{{json .Runtimes}}", check=False).stdout
    return "runsc" in out


def docker_image_exists(ref: str) -> bool:
    return docker("image", "inspect", ref, check=False).returncode == 0


def require_image(ref: str) -> None:
    import pytest

    if not docker_image_exists(ref):
        pytest.skip(
            f"image {ref} is not present; build it with "
            "`libs/images/build.sh linux` (or set CUA_E2E_DESKTOP_IMAGE)"
        )


# ---------------------------------------------------------------- spacesd in docker


@dataclass
class DriverContainer:
    """A linux container started directly with ``docker run``
    (the ``direct-connect`` topology: nothing but a URL and a token)."""

    name: str
    token: str
    runtime: str
    ports: dict[int, int] = field(default_factory=dict)

    @property
    def url(self) -> str:
        return f"http://127.0.0.1:{self.ports[3211]}"


@contextlib.contextmanager
def driver_container(what: str, image: Optional[str] = None) -> Iterator[DriverContainer]:
    image = image or desktop_image()
    require_image(image)
    runtime = "runsc" if docker_has_runsc() else "runc"
    c = DriverContainer(name=name(what), token=secrets.token_hex(16), runtime=runtime)
    docker("rm", "-f", c.name, check=False)
    try:
        docker(
            "run",
            "-d",
            "--name",
            c.name,
            f"--runtime={runtime}",
            "--memory=2g",
            "--memory-swap=2g",
            "--shm-size=512m",
            "--label",
            f"cua-e2e-run={RUN}",
            "-e",
            f"CUA_ENV_TOKEN={c.token}",
            "-p",
            "127.0.0.1::3211",
            image,
        )
        for port in (3211,):
            out = docker("port", c.name, f"{port}/tcp").stdout.strip().splitlines()[0]
            c.ports[port] = int(out.rsplit(":", 1)[1])
        yield c
    finally:
        docker("rm", "-f", c.name, check=False)


# ---------------------------------------------------------------- the conformance smoke


ENV_RETRY = (cua.CuaError.SpacesdNotAvailable, cua.CuaError.Transport, cua.CuaError.Timeout)


async def wait_env(sb: "cua.Sandbox", attempts: int = 120) -> "cua.SpacesdClient":
    """``sb.spacesd()`` retried while the driver is still starting (bounded)."""
    return await poll(
        "spacesd", lambda: sb.spacesd(5_000), attempts=attempts, delay=1, retry=ENV_RETRY
    )


async def env_smoke(env: "cua.SpacesdClient", *, desktop: bool, mock: bool = False) -> dict:
    """The spacesd smoke every topology runs. Returns a normalized,
    topology-independent summary (compared across daemon/embedded)."""
    caps = await env.capabilities()
    assert caps.version, caps
    out = await env.run(cua.SpacesdCommand(program="echo", args=["hi"]))
    assert out.exit.success and out.stdout == b"hi\n", out
    # The MockServer scripts `fail <code>`; a real guest uses the shell.
    fail = await env.sh("fail 4" if mock else "exit 4", None)
    assert fail.exit.code == 4, fail.exit

    proc = await env.spawn(cua.SpacesdCommand(program="cat", args=[], stdin=True))
    await proc.write_stdin(b"xyz")
    await proc.close_stdin()
    assert (await proc.wait()).stdout == b"xyz"

    blob = bytes(i % 251 for i in range(1 << 20))
    path = f"/tmp/cua-e2e-{RUN}/blob.bin"
    up = await env.upload(path, blob, None)
    assert up.size == len(blob)
    assert await env.download(path) == blob
    st = await env.stat(path)
    assert st.size == len(blob) and st.kind == "file", st

    with contextlib.suppress(cua.CuaError.NotFound):
        await env.download("/definitely/not/here")
        raise AssertionError("download of a missing file must be NotFound")
    health = json.loads(await env.call_json("SystemService/Health", "{}"))

    summary = {
        "echo": out.stdout.decode(),
        "exit4": fail.exit.code,
        "stdin_roundtrip": "xyz",
        "blob_size": up.size,
        "health_keys": sorted(health.keys()),
        "os_family": caps.os_family,
    }
    if desktop:
        await env.set_clipboard("cua-e2e clipboard")
        assert await env.get_clipboard() == "cua-e2e clipboard"
        shot = await env.screenshot(None)
        assert is_png(shot.image) and shot.width > 0 and shot.height > 0, shot.format
        if mock:
            await env.move_to(10.0, 10.0)
        else:
            # A screen-space move with no window must move the real pointer
            # (foreground XTest). A spacesd older than cua-driver
            # 7dcaf8165 sent it as a background XSendEvent to the window under
            # the point until the GTK desktop mapped, so the pointer stayed at
            # the X server's initial centre (the old 640,400 flake).
            moved = json.loads(
                await env.pointer_json(json.dumps({"move": {"position": {"x": 10.0, "y": 10.0}}}))
            )
            assert moved.get("report", {}).get("pointerMoved"), (
                f"the move did not move the pointer: {moved}. The image's spacesd predates "
                "the AUTO screen-space fix; rebuild it (tests/e2e/cua-sdk/ci-setup.sh --images)"
            )
        pos = await env.cursor_position()
        summary.update(
            clipboard="cua-e2e clipboard",
            screenshot_png=True,
            screen=(shot.width, shot.height),
            cursor=(round(pos.x), round(pos.y)),
        )
    return summary


# ---------------------------------------------------------------- legacy computer-server (/cmd)


def parse_cmd_sse(body: bytes) -> dict:
    """computer-server's ``POST /cmd`` answers with one SSE ``data:`` frame."""
    for line in body.decode(errors="replace").splitlines():
        if line.startswith("data: "):
            return json.loads(line[6:])
    raise AssertionError(f"no data frame in /cmd response: {body[:200]!r}")


async def legacy_cmd(
    sb: "cua.Sandbox", service: str, command: str, params: Optional[dict] = None
) -> dict:
    body = json.dumps({"command": command, "params": params or {}}).encode()
    resp = await sb.service(service).request("POST", "/cmd", body, 120_000)
    assert 200 <= resp.status < 300, (resp.status, resp.body[:300])
    payload = parse_cmd_sse(resp.body)
    assert payload.get("success", True), payload
    return payload


def fleet_client(tmp: Path) -> "cua.Cua":
    return cua.embedded(state_dir=str(tmp))


@contextlib.contextmanager
def timer() -> Iterator[list]:
    t = [time.monotonic()]
    yield t
    t.append(time.monotonic())


# ---------------------------------------------------------------- cua daemon


@dataclass
class Daemon:
    socket: Path
    home: Path
    proc: subprocess.Popen

    def client(self) -> "cua.Cua":
        return cua.connect(str(self.socket))


@contextlib.contextmanager
def cua_daemon() -> Iterator[Daemon]:
    """``cua daemon start --foreground`` on a private socket and state dir.
    HOME stays real so the container engine is found; CUA_HOME is private.
    The socket lives under /tmp because macOS caps socket paths at 104 bytes."""
    import tempfile

    import pytest

    binary = cua_cli()
    if binary is None:
        pytest.skip("the cua CLI is not built")
    home = Path(tempfile.mkdtemp(prefix=f"cua-e2e-{RUN}-", dir="/tmp"))
    sock = home / "cua.sock"
    env = dict(os.environ, CUA_HOME=str(home))
    proc = subprocess.Popen(
        [
            str(binary),
            "daemon",
            "start",
            "--foreground",
            "--socket",
            str(sock),
            "--state-dir",
            str(home / "sandboxes"),
        ],
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=open(home / "daemon.log", "w"),
    )
    try:
        for _ in range(150):  # 15 s, bounded
            if sock.exists() or proc.poll() is not None:
                break
            time.sleep(0.1)
        if not sock.exists():
            raise AssertionError(
                f"daemon did not start: {(home / 'daemon.log').read_text()[-2000:]}"
            )
        yield Daemon(socket=sock, home=home, proc=proc)
    finally:
        if proc.poll() is None:
            with contextlib.suppress(Exception):
                run_async(cua.connect(str(sock)).shutdown_daemon(), timeout=15)
            try:
                proc.wait(timeout=15)
            except subprocess.TimeoutExpired:
                proc.terminate()
                proc.wait(timeout=15)
        shutil.rmtree(home, ignore_errors=True)


# ---------------------------------------------------------------- desktop checks (run-omarchy / viewer)


async def sh_ok(env: "cua.SpacesdClient", line: str, timeout_ms: int = 60_000) -> str:
    out = await env.sh(line, timeout_ms)
    assert out.exit.success, (line, out.exit, out.stderr.decode(errors="replace")[-500:])
    return out.stdout.decode(errors="replace")


async def fixture_log(env: "cua.SpacesdClient", name: str) -> list[dict]:
    try:
        raw = await env.download(f"/tmp/cua-fixtures/{name}.jsonl")
    except cua.CuaError.NotFound:
        return []
    return [json.loads(ln) for ln in raw.decode(errors="replace").splitlines() if ln.strip()]


async def window_origin(env: "cua.SpacesdClient", title: str) -> tuple[int, int]:
    """Top-left of a fixture window's client area, via xdotool/xwininfo in
    the guest session (the fixtures log in window coordinates)."""
    # Wait until the window manager manages it (WM_STATE) and its origin is
    # stable: a fixture logs `ready` before the WM reparents and places the
    # window, and input sent in between is lost.
    script = (
        f'wid=$(xdotool search --name "{title}" | head -1); [ -n "$wid" ] || exit 3; '
        'for i in $(seq 1 50); do xprop -id "$wid" WM_STATE 2>/dev/null | grep -q "window state" && break; sleep 0.1; done; '
        'xdotool windowactivate --sync "$wid" >/dev/null 2>&1 || true; xdotool windowraise "$wid"; '
        "o=; for i in $(seq 1 20); do sleep 0.3; "
        'n=$(xwininfo -id "$wid" | awk "/Absolute upper-left X/{x=\\$4} /Absolute upper-left Y/{y=\\$4} END{print x, y}"); '
        '[ "$n" = "$o" ] && break; o=$n; done; echo "$n"'
    )
    out = await poll(
        f"window {title!r}",
        lambda: env.sh(f"desktop-env bash -c '{script}'", 20_000),
        attempts=30,
        delay=1,
    )
    x, y = out.stdout.decode().split()
    return int(x), int(y)


async def desktop_checks(env: "cua.SpacesdClient") -> dict:
    """run-omarchy: dimensions, screenshot, clipboard, click and keypress
    verified by the grid fixture's own event log."""
    shot = await env.screenshot(None)
    assert is_png(shot.image) and png_size(shot.image) == (shot.width, shot.height)
    displays = json.loads(await env.displays())
    await env.set_clipboard("hello from cua-e2e")
    assert await env.get_clipboard() == "hello from cua-e2e"

    await sh_ok(env, "cua-fixtures start grid")
    gx, gy = await window_origin(env, "CUA Fixture Grid")
    # Centre of grid cell (2, 3): cells are 80 px. Explicit foreground
    # delivery here; test_desktop.py::test_typed_click_reaches_gtk covers the
    # typed click() (DELIVERY_AUTO).
    await click_foreground(env, gx + 2 * 80 + 40, gy + 3 * 80 + 40)

    async def pressed():
        return [
            e
            for e in await fixture_log(env, "grid")
            if e.get("type") == "button_press" and e.get("cell") == [2, 3]
        ]

    await poll("grid button_press in cell [2,3]", pressed, attempts=20, delay=0.5)
    await env.press("a")
    await env.hotkey(["ctrl", "b"])

    async def keys():
        ev = [e for e in await fixture_log(env, "grid") if e.get("type") == "key_press"]
        names = [e.get("key") for e in ev]
        return ev if "a" in names and "b" in names else None

    ev = await poll("grid key_press a and ctrl+b", keys, attempts=20, delay=0.5)
    ctrl_b = [e for e in ev if e.get("key") == "b"][0]
    assert "ctrl" in ctrl_b.get("mods", []), ctrl_b
    with contextlib.suppress(Exception):
        await env.sh("cua-fixtures stop grid", 10_000)
    return {"screen": (shot.width, shot.height), "displays": displays, "click_cell": [2, 3]}


async def click_foreground(env: "cua.SpacesdClient", x: float, y: float) -> dict:
    return json.loads(
        await env.pointer_json(
            json.dumps(
                {
                    "target": {"delivery": "DELIVERY_FOREGROUND"},
                    "click": {"position": {"x": x, "y": y}},
                }
            )
        )
    )


def mcp_initialize(addr: str, token: str) -> dict:
    status, headers, body = http_post_json(
        f"http://{addr}/mcp",
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {"name": "cua-e2e", "version": "0.1.0"},
            },
        },
        headers={"authorization": f"Bearer {token}"},
    )
    assert status == 200, (status, body[:300])
    text = body.decode()
    if text.lstrip().startswith("data:") or "\ndata:" in text:
        text = [ln[5:].strip() for ln in text.splitlines() if ln.startswith("data:")][0]
    msg = json.loads(text)
    assert msg.get("result", {}).get("serverInfo"), msg
    return msg["result"]

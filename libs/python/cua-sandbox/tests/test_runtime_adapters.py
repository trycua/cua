"""The Docker/QEMU/Lume runtimes are adapters over the cua SDK's local runtimes.

A recording fake stands in for ``cua.Cua.sandboxes()`` so these run without a
container engine or VM; tests/live/test_local_spacesd.py runs the real
thing.
"""

from __future__ import annotations

import json
from types import SimpleNamespace

import pytest
from cua_sandbox import Image, sandbox_state
from cua_sandbox.runtime import native as native_module
from cua_sandbox.runtime.docker import DockerRuntime
from cua_sandbox.runtime.lume import LumeRuntime
from cua_sandbox.runtime.qemu import (
    QEMUBaremetalRuntime,
    QEMUDockerRuntime,
    _native_eligible,
)


class FakeSandboxes:
    def __init__(self, state_dir):
        self.state_dir = state_dir
        self.created = []
        self.deleted = []

    async def create(self, options):
        self.created.append(options)
        # The SDK persists the record the way sandbox_state reads it.
        (self.state_dir / f"{options.name}.json").write_text(
            json.dumps(
                {
                    "name": options.name,
                    "runtime_type": "container",
                    "host": "127.0.0.1",
                    "api_port": 45000,
                    "exposed_ports": {"3211": 45000, "8080": 45001},
                    "services": {"env": 3211},
                    "os_type": options.os,
                    "status": "running",
                }
            )
        )
        return SimpleNamespace(info=lambda: None)

    async def delete(self, name):
        self.deleted.append(name)


@pytest.fixture
def sdk(monkeypatch, tmp_path):
    monkeypatch.setattr(sandbox_state, "SANDBOX_STATE_DIR", tmp_path)
    fake = FakeSandboxes(tmp_path)
    monkeypatch.setattr(
        native_module, "local_runtime", lambda: SimpleNamespace(sandboxes=lambda: fake)
    )
    return fake


async def test_docker_runtime_starts_a_container_through_the_sdk(sdk):
    image = Image.from_registry("registry.example/desktop:1", kind="container").expose(8080)

    info = await DockerRuntime(memory_mb=2048, cpus=2).start(image, "cua-e2e-a")

    options = sdk.created[0]
    assert options.image == "container:registry.example/desktop:1"
    assert options.name == "cua-e2e-a"
    assert (options.memory_mb, options.cpus) == (2048, 2)
    assert options.ports == [3211, 8080]
    # `env` is implicit: the SDK reports it only when the image runs cua-spacesd.
    assert options.services == {}
    assert options.wait_for == [], "readiness is daemon-agnostic by default"
    assert options.token and options.env["CUA_ENV_TOKEN"] == options.token
    # RuntimeInfo points at the published env port and keeps the token.
    assert (info.host, info.api_port, info.guest_server_port) == ("127.0.0.1", 45000, 3211)
    assert info.exposed_ports == {8080: 45001}
    assert info.env_token == options.token
    assert sandbox_state.load("cua-e2e-a")["env_token"] == options.token


async def test_a_declared_server_port_becomes_the_readiness_probe(sdk):
    image = Image.from_registry("registry.example/app:1", kind="container")

    await DockerRuntime(server_port=8000).start(image, "cua-e2e-b")

    options = sdk.created[0]
    assert 8000 in options.ports
    assert [(p.port, p.http_path) for p in options.wait_for] == [(8000, None)]
    # ...and is reachable by name (sb.services.request("server", ...)).
    assert options.services == {"server": 8000}


async def test_a_sandbox_without_spacesd_reports_no_env_port(sdk):
    """The SDK records no `env` service for an image without cua-spacesd."""
    from cua_sandbox.runtime.native import runtime_info_from_state

    state = {"host": "127.0.0.1", "api_port": 45001, "exposed_ports": {"8080": 45001}}
    info = runtime_info_from_state("plain", {**state, "services": {"port-8080": 8080}})
    assert info.api_port == 0 and info.guest_server_port is None
    assert info.exposed_ports == {8080: 45001}
    # Older state files (no services recorded) keep the spacesd default.
    legacy = runtime_info_from_state("old", {**state, "exposed_ports": {"3211": 45000}})
    assert (legacy.api_port, legacy.guest_server_port) == (45000, 3211)


def test_docker_runtime_rejects_host_escape_hatches():
    with pytest.raises(ValueError, match="volumes, privileged"):
        DockerRuntime(volumes=["/:/host"], privileged=True)


async def test_ephemeral_docker_stop_deletes(sdk):
    await DockerRuntime().stop("cua-e2e-c")
    assert sdk.deleted == ["cua-e2e-c"]


async def test_qemu_boots_the_pinned_container_disk_through_the_sdk(sdk):
    await QEMUBaremetalRuntime(cpu_count=4, memory_mb=4096).start(
        Image.linux(kind="vm"), "cua-e2e-d"
    )

    options = sdk.created[0]
    # The canonical image; the SDK's resolver boots its `-disk` containerDisk.
    assert options.image == "vm:ghcr.io/trycua/linux:24.04"
    assert (options.os, options.cpus, options.memory_mb) == ("linux", 4, 4096)


async def test_qemu_boots_a_local_disk_through_the_sdk(sdk, tmp_path):
    disk = tmp_path / "disk.qcow2"
    disk.write_bytes(b"qcow2")

    await QEMUBaremetalRuntime().start(Image.from_file(str(disk)), "cua-e2e-e")

    assert sdk.created[0].image == f"disk:{disk}"


async def test_qemu_docker_mode_is_the_same_sdk_backend(sdk):
    await QEMUDockerRuntime().start(Image.windows(), "cua-e2e-f")
    assert sdk.created[0].image.startswith("vm:") and sdk.created[0].os == "windows"

    with pytest.raises(NotImplementedError, match="AndroidEmulatorRuntime"):
        await QEMUDockerRuntime().start(Image.android("14"), "cua-e2e-g")


@pytest.mark.parametrize(
    "image, opts, use_qmp, expected",
    [
        (Image.linux(), {}, False, True),
        (Image.windows(), {}, False, True),
        (Image.android("14"), {}, False, False),  # Android-x86 stays legacy
        (Image.linux(), {}, True, False),  # QMP-only transport stays legacy
        (Image.linux(), {"disk_path": "/tmp/install.iso"}, False, False),  # ISO install
    ],
)
def test_which_qemu_launches_stay_on_the_legacy_launcher(image, opts, use_qmp, expected):
    assert _native_eligible(image, opts, use_qmp=use_qmp) is expected


def test_legacy_launcher_can_be_forced(monkeypatch):
    monkeypatch.setenv("CUA_SANDBOX_LEGACY_QEMU", "1")
    assert _native_eligible(Image.linux(), {}) is False


async def test_lume_runtime_uses_the_sdk_lume_backend(sdk, monkeypatch):
    delivered = []

    async def deliver(self, name, lume_url):
        delivered.append((name, lume_url))

    monkeypatch.setattr(LumeRuntime, "_deliver_vnc_config", deliver)

    await LumeRuntime().start(Image.macos("26"), "cua-e2e-h")

    options = sdk.created[0]
    assert options.image == "lume:ghcr.io/trycua/macos:26"
    assert options.os == "macos"
    assert delivered == [("cua-e2e-h", "http://localhost:7777")]


async def test_vm_backends_do_not_mint_a_token_the_guest_never_sees(sdk, monkeypatch):
    """The SDK's VM backends do not pass the sandbox env into the guest yet,
    so a minted CUA_ENV_TOKEN would only lock the client out."""
    monkeypatch.delenv("CUA_SANDBOX_ENV_TOKEN", raising=False)
    await QEMUBaremetalRuntime().start(Image.linux(), "cua-e2e-i")
    assert sdk.created[0].token is None
    assert "CUA_ENV_TOKEN" not in sdk.created[0].env

    monkeypatch.setenv("CUA_SANDBOX_ENV_TOKEN", "baked-into-image")
    await QEMUBaremetalRuntime().start(Image.linux(), "cua-e2e-j")
    assert sdk.created[1].token == "baked-into-image"

"""The unified Sandbox API: command/env/services/wait_for, portable info,
service handles and public URLs, hermetically.

Runtimes are recorders (nothing is launched).
"""

from __future__ import annotations

import warnings
from typing import Any

import pytest
from cua_sandbox import Image, Sandbox, http, tcp
from cua_sandbox.options import check_services, parse_memory, probes
from cua_sandbox.runtime.native import NativeRuntime


def test_sandbox_info_location_kind_and_runtime():
    from cua_sandbox.sandbox import SandboxInfo

    info = SandboxInfo(
        name="a",
        status="ready",
        source="container",
        location="local",
        kind="container",
        runtime="gvisor",
    )
    assert (info.location, info.kind, info.runtime, info.id) == (
        "local",
        "container",
        "gvisor",
        "local:a",
    )
    assert not hasattr(info, "where"), "SandboxInfo.where never shipped"


# ── options ──────────────────────────────────────────────────────────────


def test_probes_memory_and_service_validation():
    assert tcp("mcp").path is None
    assert http("mcp", "health").path == "/health"
    assert probes(None) == []
    assert probes(tcp("a")) == [tcp("a")]
    assert probes([tcp("a"), http("b")]) == [tcp("a"), http("b", "/")]
    with pytest.raises(TypeError):
        probes([8765])  # type: ignore[list-item]
    assert parse_memory("4GB") == 4096
    assert parse_memory("512MB") == 512
    assert parse_memory(2048) == 2048
    assert parse_memory("2") == 2048
    with pytest.raises(ValueError):
        parse_memory("lots")
    assert check_services({"mcp": 8765}, [tcp("mcp")]) == {"mcp": 8765}
    with pytest.raises(ValueError, match="not declared"):
        check_services({"mcp": 8765}, [tcp("web")])
    with pytest.raises(ValueError, match="port"):
        check_services({"mcp": 0}, [])


def test_native_probe_names_the_service():
    p = http("mcp", "/health").native()
    assert (p.port, p.service, p.http_path) == (0, "mcp", "/health")


# ── local: options reach the runtime ────────────────────────────────────


class _Stop(Exception):
    pass


def _native_recorder(calls: list):
    class Recorder(NativeRuntime):
        def __init__(self, *args: Any, **kwargs: Any) -> None:
            super().__init__(**kwargs)

        async def start(self, image: Image, name: str, **opts: Any) -> Any:
            calls.append(opts)
            raise _Stop()

    return Recorder


async def test_local_create_passes_command_env_services_and_probes(monkeypatch):
    calls: list = []
    monkeypatch.setattr("cua_sandbox.runtime.docker.DockerRuntime", _native_recorder(calls))
    with pytest.raises(_Stop):
        await Sandbox.create(
            Image.linux(kind="container"),
            local=True,
            command=["python", "-m", "srv"],
            env={"K": "v"},
            services={"mcp": 8765},
            wait_for=http("mcp", "/health"),
            memory="2GB",
            telemetry_enabled=False,
        )
    [opts] = calls
    assert opts["command"] == ["python", "-m", "srv"]
    assert opts["env"] == {"K": "v"}
    assert opts["services"] == {"mcp": 8765}
    assert opts["wait_for"] == [http("mcp", "/health")]
    assert opts["memory_mb"] == 2048


# ── where a sandbox runs: local by default ───────────────────────────────


@pytest.fixture
def fresh_notice(monkeypatch):
    import sys

    from cua_sandbox import _placement

    monkeypatch.setattr(_placement, "_default_notice_shown", False)
    monkeypatch.delenv("CUA_QUIET_DEFAULT", raising=False)
    return sys.modules["cua_sandbox.sandbox"]


async def test_local_is_the_default_with_a_one_time_notice(monkeypatch, fresh_notice, caplog):
    calls: list = []
    monkeypatch.setattr("cua_sandbox.runtime.docker.DockerRuntime", _native_recorder(calls))
    caplog.set_level("WARNING", logger="cua_sandbox.sandbox")
    for _ in range(2):
        with warnings.catch_warnings():
            warnings.simplefilter("error", DeprecationWarning)
            with pytest.raises(_Stop):
                await Sandbox.create(
                    Image.linux(kind="container"), services={"web": 80}, telemetry_enabled=False
                )
    assert len(calls) == 2, "both went to the local runtime"
    notices = [r for r in caplog.records if "defaulting to a local sandbox" in r.message]
    assert len(notices) == 1, "the notice is shown once"
    assert "CUA_QUIET_DEFAULT=1" in notices[0].message


async def test_quiet_default_hides_the_notice(monkeypatch, fresh_notice, caplog):
    monkeypatch.setenv("CUA_QUIET_DEFAULT", "1")
    monkeypatch.setattr("cua_sandbox.runtime.docker.DockerRuntime", _native_recorder([]))
    caplog.set_level("WARNING", logger="cua_sandbox.sandbox")
    with pytest.raises(_Stop):
        await Sandbox.create(Image.linux(kind="container"), telemetry_enabled=False)
    assert not [r for r in caplog.records if "defaulting to a local" in r.message]


async def test_native_runtime_start_builds_the_sdk_options(monkeypatch):
    seen: dict = {}

    class Sandboxes:
        async def create(self, options):
            seen["options"] = options
            raise _Stop()

    class Runtime:
        def sandboxes(self):
            return Sandboxes()

    monkeypatch.setattr("cua_sandbox.runtime.native.local_runtime", lambda: Runtime())

    class Plain(NativeRuntime):
        async def _image_ref(self, image, name, **opts):
            return "container:python:3.12-slim"

    with pytest.raises(_Stop):
        await Plain().start(
            Image.linux(kind="container"),
            "cua-e2e-opts",
            command=["python", "/srv.py"],
            env={"K": "v"},
            services={"mcp": 8765},
            wait_for=[tcp("mcp")],
        )
    o = seen["options"]
    assert o.command == ["python", "/srv.py"]
    assert o.env["K"] == "v"
    # `env` is implicit: the SDK reports it only for images with cua-spacesd.
    assert o.services == {"mcp": 8765}
    assert 8765 in o.ports
    assert [(p.service, p.http_path) for p in o.wait_for] == [("mcp", None)]


def _capturing_runtime(monkeypatch, seen: dict):
    class Sandboxes:
        async def create(self, options):
            seen["options"] = options
            raise _Stop()

    class Runtime:
        def sandboxes(self):
            return Sandboxes()

    monkeypatch.setattr("cua_sandbox.runtime.native.local_runtime", lambda: Runtime())


async def test_local_layers_on_a_container_image_build_before_boot(monkeypatch, tmp_path):
    seen: dict = {}
    _capturing_runtime(monkeypatch, seen)

    async def no_post_boot(self, info, image):  # pragma: no cover - must not run
        raise AssertionError("container layers are built, not applied after boot")

    monkeypatch.setattr(NativeRuntime, "_apply_layers", no_post_boot)

    class Plain(NativeRuntime):
        async def _image_ref(self, image, name, **opts):
            return "container:python:3.12-slim"

    f = tmp_path / "app.py"
    f.write_text("print('hi')\n")
    image = (
        Image.from_registry("python:3.12-slim")
        .pip_install("mcp")
        .run("echo built > /built")
        .env(GREETING="hello")
        .copy(str(f), "/srv/app.py")
    )
    with pytest.raises(_Stop):
        await Plain().start(image, "cua-e2e-layers")
    build = seen["options"].build
    assert build is not None, "layers go to the local build"
    assert [type(layer).__name__.rsplit(".", 1)[-1] for layer in build.layers] == [
        "PIP_INSTALL",
        "RUN",
    ]
    assert build.env == {"GREETING": "hello"}
    assert [(b.source, b.destination) for b in build.files] == [(str(f), "/srv/app.py")]
    assert seen["options"].image == "container:python:3.12-slim"


async def test_local_layers_on_a_vm_need_spacesd(monkeypatch):
    from cua_sandbox import Unsupported
    from cua_sandbox._sdk import native

    seen: dict = {}
    _capturing_runtime(monkeypatch, seen)
    n = native()

    class Resolved:
        spacesd = False

    monkeypatch.setattr(n, "resolve_image", lambda ref, backend, arch: Resolved())

    class Vm(NativeRuntime):
        async def _image_ref(self, image, name, **opts):
            return "vm:registry.example/plain-disk:1"

    image = Image.from_registry("registry.example/plain-disk:1", kind="vm").run("true")
    with pytest.raises(Unsupported, match="cua-spacesd"):
        await Vm().start(image, "cua-e2e-vm-layers")
    assert "options" not in seen, "refused before anything boots"
    # With spacesd, the VM boots without a build and takes them after boot.
    Resolved.spacesd = True
    with pytest.raises(_Stop):
        await Vm().start(image, "cua-e2e-vm-layers")
    assert seen["options"].build is None
    # VM-only layers never go to a container build...
    with pytest.raises(Unsupported, match="app_install"):
        from cua_sandbox.containers import image_build

        image_build(Image.linux().app_install("firefox"))

    # ...on a container image they are applied after boot when it runs
    # cua-spacesd (Image.linux()), and refused when it does not.
    class Ctr(NativeRuntime):
        async def _image_ref(self, image, name, **opts):
            return "container:ghcr.io/trycua/linux:24.04"

    seen.clear()
    app = Image.linux(kind="container").app_install("firefox")
    with pytest.raises(_Stop):
        await Ctr().start(app, "cua-e2e-ctr-app")
    assert seen["options"].build is None
    Resolved.spacesd = False
    seen.clear()
    with pytest.raises(Unsupported, match="cua-spacesd"):
        await Ctr().start(app, "cua-e2e-ctr-app")
    assert "options" not in seen


async def test_non_sdk_runtimes_refuse_the_portable_options():
    class Legacy:
        async def start(self, *a, **k):  # pragma: no cover - never reached
            raise AssertionError

    with pytest.raises(NotImplementedError, match="command, env, services"):
        await Sandbox.create(
            Image.linux(kind="container"),
            local=True,
            runtime=Legacy(),  # type: ignore[arg-type]
            services={"mcp": 8765},
            telemetry_enabled=False,
        )

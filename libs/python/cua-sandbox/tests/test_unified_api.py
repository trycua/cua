"""The unified Sandbox API: command/env/services/wait_for, CloudOptions,
portable info, service handles and public URLs, hermetically.

Local: runtimes are recorders (nothing is launched). Cloud: the fake Fleet
API from ``cua-test-fixtures`` (see ``tests/_native_fleet.py``).
"""

from __future__ import annotations

import warnings
from typing import Any

import pytest
from cua_sandbox import CloudOptions, Image, Sandbox, http, tcp
from cua_sandbox.options import check_services, parse_memory, probes
from cua_sandbox.runtime.native import NativeRuntime

# Digest-pinned (no registry resolution) and a rootfs (runs on gVisor).
ROOTFS = "registry.example/mcp@sha256:beef"


@pytest.fixture
def rootfs_variant(monkeypatch):
    from tests import _image_fixtures

    monkeypatch.setitem(_image_fixtures.VARIANTS, ROOTFS, "rootfs")


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
    assert "local=False" in notices[0].message


async def test_quiet_default_hides_the_notice(monkeypatch, fresh_notice, caplog):
    monkeypatch.setenv("CUA_QUIET_DEFAULT", "1")
    monkeypatch.setattr("cua_sandbox.runtime.docker.DockerRuntime", _native_recorder([]))
    caplog.set_level("WARNING", logger="cua_sandbox.sandbox")
    with pytest.raises(_Stop):
        await Sandbox.create(Image.linux(kind="container"), telemetry_enabled=False)
    assert not [r for r in caplog.records if "defaulting to a local" in r.message]


async def test_cloud_only_arguments_keep_the_cloud_with_a_deprecation(monkeypatch, fresh_notice):
    seen: list = []

    async def managed(image, **kwargs):
        seen.append(kwargs)
        raise _Stop()

    import sys

    monkeypatch.setattr(sys.modules["cua_sandbox.sandbox"], "_acquire_managed", managed)
    monkeypatch.setattr(Sandbox, "_uses_fleet", staticmethod(lambda api_key: True))
    with pytest.warns(DeprecationWarning, match="local=False"):
        with pytest.raises(_Stop):
            await Sandbox.create(
                Image.from_registry(ROOTFS), claim_ttl=600, telemetry_enabled=False
            )
    assert seen and seen[0]["claim_ttl"] == 600
    # cloud= implies the cloud, without a warning.
    with warnings.catch_warnings():
        warnings.simplefilter("error", DeprecationWarning)
        with pytest.raises(_Stop):
            await Sandbox.create(
                Image.from_registry(ROOTFS),
                cloud=CloudOptions(max_pool_size=2),
                telemetry_enabled=False,
            )
    assert len(seen) == 2


async def test_local_true_with_cloud_options_is_a_typed_error():
    from cua_sandbox import InvalidArgument

    with pytest.raises(InvalidArgument, match="contradict"):
        await Sandbox.create(
            Image.linux(kind="container"),
            local=True,
            cloud=CloudOptions(warm=True),
            telemetry_enabled=False,
        )
    with pytest.raises(ValueError):
        async with Sandbox.ephemeral(
            Image.linux(kind="container"), local=True, cloud=CloudOptions()
        ):
            pass


async def test_cloud_suspend_is_unsupported_and_never_scales_a_pool(monkeypatch, tmp_path):
    from cua_sandbox import Unsupported, sandbox_state

    monkeypatch.setattr(Sandbox, "_uses_fleet", staticmethod(lambda api_key: True))
    monkeypatch.setattr(sandbox_state, "load", lambda name: None)

    async def gone(cls, **kwargs):
        raise RuntimeError("no such claim")

    monkeypatch.setattr(Sandbox, "_create", classmethod(gone))
    for op in ("suspend", "resume", "restart"):
        with pytest.raises(Unsupported, match="cannot suspend a single sandbox"):
            await getattr(Sandbox, op)("some-claim")
        # Still a NotImplementedError for older handlers.
        with pytest.raises(NotImplementedError):
            await getattr(Sandbox, op)("some-claim", local=False)

    # A running cloud sandbox resumes by reconnecting.
    async def live(cls, **kwargs):
        return ("connected", kwargs["name"])

    monkeypatch.setattr(Sandbox, "_create", classmethod(live))
    assert await Sandbox.resume("some-claim") == ("connected", "some-claim")
    from cua_sandbox.transport.fleet_cloud import FleetCloudTransport

    assert not hasattr(FleetCloudTransport, "suspend_sandbox"), "no pool scaling path"


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

        image_build(Image.linux().app_install("firefox"), where="local")

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


# ── deprecated keywords ─────────────────────────────────────────────────


async def test_flat_cloud_keywords_are_deprecated_and_conflicts_refused(monkeypatch):
    calls: list = []
    monkeypatch.setattr("cua_sandbox.runtime.docker.DockerRuntime", _native_recorder(calls))
    with pytest.warns(DeprecationWarning, match=r"cloud=CloudOptions\(warm=\.\.\."):
        with pytest.raises(_Stop):
            await Sandbox.create(
                Image.linux(kind="container"), local=True, warm=True, telemetry_enabled=False
            )
    with pytest.raises(ValueError, match="both in cloud="):
        with warnings.catch_warnings():
            warnings.simplefilter("ignore", DeprecationWarning)
            await Sandbox.create(
                Image.linux(kind="container"),
                warm=True,
                cloud=CloudOptions(warm=False),
                telemetry_enabled=False,
                local=False,
            )
    # A named pool: the given fields are compared with its template.
    from cua_sandbox import Pool, PoolSpecMismatch

    async def check(name, spec):
        raise PoolSpecMismatch(f"pool {name}: command: pool has [], requested {spec.command}")

    monkeypatch.setattr(Pool, "check", staticmethod(check))
    with pytest.raises(PoolSpecMismatch, match="command"):
        await Sandbox.create(
            cloud=CloudOptions(pool="p"), command=["x"], telemetry_enabled=False, local=False
        )


# ── cloud: the fake Fleet API ────────────────────────────────────────────


async def test_cloud_command_services_info_urls_and_progress(fleet, rootfs_variant):
    events: list = []
    sb = await Sandbox.create(
        Image.from_registry(ROOTFS),
        command=["python", "/srv.py"],
        services={"mcp": 8765},
        cloud=CloudOptions(max_pool_size=3, claim_ttl=600),
        progress=events.append,
        telemetry_enabled=False,
        local=False,
    )
    try:
        # The command is part of the template (and the pool key).
        client = fleet()
        try:
            template = await client.get_template(sb.pool_name, sb.pool_name)
        finally:
            await client.close()
        vm = template.spec.vm_template
        assert list(vm.command) == ["python", "/srv.py"]
        assert {s.name: s.target_port for s in vm.services}["mcp"] == 8765
        # Portable progress: no pool/claim words.
        stages = [e.stage for e in events]
        assert stages[0] == "provisioning" and stages[-1] == "ready", stages
        words = [e.message.replace(sb.claim_name or "", "<id>") for e in events]
        assert all("pool" not in m and "claim" not in m for m in words), words
        assert "first start of an image can take a few minutes" in events[0].message
        # Portable identity and info.
        # The qualified ref: no pool or namespace in it.
        assert sb.id == f"cloud:{sb.claim_name}"
        assert sb.location == "cloud"
        info = await sb.info()
        assert (info.location, info.status) == ("cloud", "ready")
        assert info.services.get("mcp") == 8765
        assert info.expires_at is not None
        assert info.provider_details["pool"] == sb.pool_name
        with pytest.warns(DeprecationWarning):
            assert info.pool == sb.pool_name
        # URLs: signed in the cloud.
        url = await sb.service("mcp").url()
        assert url.startswith("https://signed.fleet.test/"), url
        public = await sb.public_url("mcp", ttl=600)
        assert public.url.startswith("https://signed.fleet.test/")
        assert public.service == "mcp" and public.expires_at.endswith("Z")
        assert "claim" in public.provider_details
        with pytest.raises(Exception, match="ttl"):
            await sb.public_url("mcp", ttl=5)
    finally:
        await sb.close()


async def test_cloud_env_is_created_and_keys_its_own_pool(fleet, rootfs_variant):
    # Cloud env= is the template's env (processMode Run on every runtime).
    plain = await Sandbox.create(Image.from_registry(ROOTFS), telemetry_enabled=False, local=False)
    sb = await Sandbox.create(
        Image.from_registry(ROOTFS), env={"K": "v"}, telemetry_enabled=False, local=False
    )
    try:
        assert sb.pool_name != plain.pool_name, "env is part of the pool key"
        assert sb.location == "cloud"
    finally:
        await sb.close()
        await plain.close()


# ── listing: local by default, all=True merges cloud ────────────────────


async def test_list_is_everything_by_default_and_the_cloud_never_fails_it(monkeypatch):
    import asyncio
    import sys

    from cua_sandbox import InvalidArgument
    from cua_sandbox.sandbox import SandboxInfo

    mod = sys.modules["cua_sandbox.sandbox"]
    local = [SandboxInfo(name="l1", status="running", source="container", location="local")]
    cloud = [SandboxInfo(name="c1", status="running", source="fleet", location="cloud")]
    calls: list = []

    async def list_local(cls):
        calls.append("local")
        return list(local)

    async def list_cloud(cls, *, api_key=None):
        calls.append("cloud")
        return list(cloud)

    monkeypatch.setattr(Sandbox, "_list_local", classmethod(list_local))
    monkeypatch.setattr(Sandbox, "_list_cloud", classmethod(list_cloud))
    monkeypatch.setenv("CUA_CLIENT_ID", "cua-e2e-id")
    monkeypatch.setenv("CUA_CLIENT_SECRET", "cua-e2e-secret")
    rows = await Sandbox.list()
    assert [(i.name, i.location) for i in rows] == [("l1", "local"), ("c1", "cloud")]
    assert [i.id for i in rows] == ["local:l1", "cloud:c1"]
    # A local and a cloud sandbox may share a name: both are listed.
    cloud.append(SandboxInfo(name="l1", status="running", source="fleet", location="cloud"))
    assert [i.id for i in await Sandbox.list()] == ["local:l1", "cloud:c1", "cloud:l1"]
    cloud.pop()
    calls.clear()
    assert [i.name for i in await Sandbox.list(local=True)] == ["l1"]
    assert calls == ["local"], "local only never reads the cloud"
    assert [i.name for i in await Sandbox.list(local=False)] == ["c1"]
    with pytest.warns(DeprecationWarning):
        assert len(await Sandbox.list(all=True)) == 2
    with pytest.raises(InvalidArgument), pytest.warns(DeprecationWarning):
        await Sandbox.list(all=True, local=True)

    # The cloud failing or hanging: local rows plus a warning.
    async def broken(cls, *, api_key=None):
        raise RuntimeError("fleet unreachable")

    monkeypatch.setattr(Sandbox, "_list_cloud", classmethod(broken))
    warned: list = []
    monkeypatch.setattr(mod.logger, "warning", lambda *a: warned.append(a[0] % a[1:]))
    assert [i.name for i in await Sandbox.list()] == ["l1"]
    assert warned == ["cloud sandboxes not listed: fleet unreachable"]
    with pytest.raises(RuntimeError):
        await Sandbox.list(local=False)

    async def hangs(cls, *, api_key=None):
        await asyncio.sleep(60)

    monkeypatch.setattr(Sandbox, "_list_cloud", classmethod(hangs))
    monkeypatch.setattr(mod, "_LIST_CLOUD_TIMEOUT", 0.05)
    warned.clear()
    assert [i.name for i in await Sandbox.list()] == ["l1"]
    assert warned and "did not answer" in warned[0]

    # No credentials readable without the keychain: no cloud call at all.
    monkeypatch.delenv("CUA_CLIENT_ID")
    monkeypatch.delenv("CUA_CLIENT_SECRET")
    monkeypatch.setattr("cua_sandbox._config.get_api_key", lambda override=None: None)
    monkeypatch.setattr("cua_sandbox._config.may_have_fleet_session", lambda: False)
    monkeypatch.setattr(Sandbox, "_list_cloud", classmethod(broken))
    warned.clear()
    assert [i.name for i in await Sandbox.list()] == ["l1"]
    assert warned == [], "silent without credentials"


def test_may_have_fleet_session_follows_the_marker_without_reading_the_vault(monkeypatch, tmp_path):
    pytest.importorskip("cua")
    from cua_sandbox import _config

    vault = tmp_path / "vault"
    monkeypatch.setenv("CUA_HOME", str(tmp_path / "home"))
    monkeypatch.setenv("CUA_CREDENTIAL_STORE", f"test-keychain:{vault}")
    monkeypatch.delenv("CUA_FLEET_SESSION", raising=False)
    assert not _config.may_have_fleet_session()
    (tmp_path / "home").mkdir()
    (tmp_path / "home" / "session.json").write_text(
        '{"store": "keychain", "expires_at": "2099-01-01T00:00:00Z"}'
    )
    assert _config.may_have_fleet_session()
    assert not (vault / "reads").exists(), "the vault was never read"
    monkeypatch.setenv("CUA_FLEET_SESSION", "0")
    assert not _config.may_have_fleet_session()

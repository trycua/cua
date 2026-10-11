"""P2 sandbox parity: sidecars, private registry secrets, cloud image layers
and warm capacity for the canonical images (hermetic).

The pure tests need nothing; the ``fleet`` ones run the native SDK against
the loopback fake Fleet (``cua-test-fixtures``), which admits sidecars like
Fleet does (every runtime, reserved service names). Cloud image layers still
fail with the "not deployed yet" error (Fleet's builder does not run them). The native option objects are checked by capturing what
``cua_sandbox`` hands the SDK.
"""

from __future__ import annotations

import asyncio
import importlib

import pytest
from cua_sandbox import Container, Image, RegistrySecret, Sandbox, _autopool
from cua_sandbox.containers import image_build, sidecars_of

from tests._native_fleet import fleet  # noqa: F401 - fixture

sandbox_module = importlib.import_module("cua_sandbox.sandbox")

IMAGE = "registry.example/workspace@sha256:0123"


# ── pure ─────────────────────────────────────────────────────────────────


def test_registry_secret_never_shows_its_password():
    s = RegistrySecret("me", "hunter2")
    assert "hunter2" not in repr(s) and "<redacted>" in repr(s)
    assert s.username == "me"
    assert "hunter2" not in repr(Image.from_registry("ghcr.io/me/app:1", secret=s))
    with pytest.raises(ValueError):
        RegistrySecret("me", "")
    assert repr(RegistrySecret.from_env()) == (
        "RegistrySecret.from_env('CUA_REGISTRY_USERNAME', 'CUA_REGISTRY_PASSWORD')"
    )
    assert RegistrySecret.aws_ecr("us-west-2").username is None


def test_the_secret_follows_the_image_but_is_never_serialized():
    s = RegistrySecret("me", "hunter2")
    img = Image.from_registry("ghcr.io/me/app:1", secret=s).pip_install("mcp").env(A="1")
    assert img._secret is s, "chained builders keep the secret"
    assert "secret" not in str(img.to_dict()) and "hunter2" not in str(img.to_dict())
    # Equality ignores the credential (same image either way).
    assert Image.from_registry("x:1", secret=s) == Image.from_registry("x:1")


def test_sidecars_normalize_and_validate():
    [a, b] = sidecars_of(["redis:7-alpine", Container("postgres:16", ports=[5432])])
    assert a == Container("redis:7-alpine")
    assert b.ports == [5432]
    with pytest.raises(ValueError):
        Container("redis", ports=[0])
    with pytest.raises(ValueError):
        Container("")
    with pytest.raises(TypeError):
        sidecars_of([42])


def test_native_conversions():
    n = pytest.importorskip("cua")._native
    c = Container("redis:7-alpine", ports=[6379], env={"A": "1"}, command=["redis-server"]).native()
    assert (c.image, c.ports, c.env, c.command, c.name) == (
        "redis:7-alpine",
        [6379],
        {"A": "1"},
        ["redis-server"],
        None,
    )
    basic = RegistrySecret("me", "tok", registry="ghcr.io").native()
    assert isinstance(basic, n.RegistrySecret.BASIC) and basic.registry == "ghcr.io"
    assert isinstance(RegistrySecret.from_env().native(), n.RegistrySecret.FROM_ENV)
    assert isinstance(RegistrySecret.aws_ecr().native(), n.RegistrySecret.AWS_ECR)

    img = (
        Image.from_registry("python:3.12-slim")
        .apt_install("curl")
        .pip_install("mcp")
        .run("echo hi")
        .env(K="v")
        .copy("server.py", "/srv/server.py")
        .expose(8765)
    )
    build = image_build(img)
    assert [type(layer).__name__.rsplit(".", 1)[-1] for layer in build.layers] == [
        "APT_INSTALL",
        "PIP_INSTALL",
        "RUN",
    ]
    assert build.env == {"K": "v"} and build.ports == [8765]
    assert (build.files[0].source, build.files[0].destination) == ("server.py", "/srv/server.py")
    assert image_build(Image.from_registry("python:3.12-slim")) is None
    with pytest.raises(NotImplementedError, match="brew_install"):
        image_build(Image.from_registry("python:3.12-slim")._add_layer({"type": "brew_install"}))


def test_warm_is_left_to_the_sdk_unless_chosen(monkeypatch):
    monkeypatch.delenv("CUA_FLEET_WARM", raising=False)
    assert _autopool.config().warm_set is False
    assert _autopool.config(warm=False).warm_set is True
    monkeypatch.setenv("CUA_FLEET_WARM", "1")
    cfg = _autopool.config()
    assert cfg.warm and cfg.warm_set


def test_the_native_sdk_decides_warm_for_canonical_images():
    n = pytest.importorskip("cua")._native
    assert n.image_alias("linux") is not None
    assert n.resolve_image_with_secret is not None


# ── what cua_sandbox hands the SDK ───────────────────────────────────────


class _Captured(Exception):
    pass


def _capture(monkeypatch) -> list:
    seen: list = []

    class _Sandboxes:
        async def create(self, options):
            seen.append(options)
            raise _Captured()

    class _Cua:
        def sandboxes(self):
            return _Sandboxes()

    monkeypatch.setattr(_autopool, "_native_cua", lambda: _Cua())
    monkeypatch.setattr(Sandbox, "_uses_fleet", staticmethod(lambda api_key: api_key is None))
    monkeypatch.setattr(sandbox_module, "_TELEMETRY_AVAILABLE", False)
    monkeypatch.delenv("CUA_FLEET_WARM", raising=False)
    return seen


def test_cloud_create_hands_sidecars_secret_and_layers_to_the_sdk(monkeypatch):
    pytest.importorskip("cua")
    seen = _capture(monkeypatch)
    img = Image.from_registry("ghcr.io/me/app:1", secret=RegistrySecret("me", "tok")).pip_install(
        "mcp"
    )
    with pytest.raises(_autopool.AutoPoolError):
        asyncio.run(
            Sandbox.create(
                img,
                sidecars=[Container("redis:7-alpine", ports=[6379])],
                services={"db": 6379},
                local=False,
            )
        )
    [o] = seen
    assert [s.image for s in o.sidecars] == ["redis:7-alpine"]
    assert o.registry_secret.username == "me"
    assert len(o.build.layers) == 1
    assert o.services["db"] == 6379
    assert o.cloud.warm is None, "unset: the SDK decides (canonical images warm)"
    assert (o.on, o.runtime) == ("cloud", "gvisor"), "a build's output is a container rootfs"


def test_cloud_create_passes_an_explicit_warm(monkeypatch):
    pytest.importorskip("cua")
    seen = _capture(monkeypatch)
    with pytest.raises(_autopool.AutoPoolError):
        asyncio.run(Sandbox.create(Image.from_registry(IMAGE), warm=False, local=False))
    assert seen[0].cloud.warm is False
    assert seen[0].sidecars == [] and seen[0].registry_secret is None and seen[0].build is None


def test_an_existing_pool_compares_sidecars_with_its_template(monkeypatch):
    from cua_sandbox import Pool, PoolSpecMismatch

    seen: list = []

    async def check(name, spec):
        seen.append((name, spec))
        raise PoolSpecMismatch("pool cua-e2e-x's template differs: sidecars")

    monkeypatch.setattr(Pool, "check", staticmethod(check))
    with pytest.raises(PoolSpecMismatch, match="sidecars"):
        asyncio.run(Sandbox.create(pool="cua-e2e-x", sidecars=["redis:7-alpine"], local=False))
    [(name, spec)] = seen
    assert name == "cua-e2e-x"
    assert [c.image for c in sidecars_of(spec.sidecars)] == ["redis:7-alpine"]


# ── end to end on the fake Fleet ─────────────────────────────────────────


async def test_cloud_vm_images_take_sidecars(fleet):  # noqa: F811
    # IMAGE is a KubeVirt containerDisk in the fake registry: Fleet runs its
    # sidecars in a companion pod, addressed by name like on gVisor.
    sb = await Sandbox.create(
        Image.from_registry(IMAGE),
        sidecars=[Container("redis:7-alpine", ports=[6379], name="db")],
        services={"db": 6379},
        local=False,
    )
    try:
        client = fleet()
        try:
            template = await client.get_template(sb.pool_name, sb.pool_name)
        finally:
            await client.close()
        assert str(template.spec.vm_template.runtime).lower().endswith("kubevirt")
        assert "db" in [s.name for s in template.spec.vm_template.services]
    finally:
        await sb.close()


@pytest.mark.parametrize("reserved", ["main", "sidecars", "sc"])
async def test_sidecars_reserve_service_names_before_creating_anything(
    fleet, reserved  # noqa: F811 - fixture
):
    with pytest.raises(Exception, match="reserved"):
        await Sandbox.create(
            Image.from_registry(IMAGE),
            sidecars=[Container("redis:7-alpine", ports=[6379], name="db")],
            services={reserved: 6379},
            local=False,
        )
    client = fleet()
    try:
        names = [n.name for n in await client.list_namespaces()]
    finally:
        await client.close()
    assert not [n for n in names if n.startswith("cua-auto-")], "nothing was created"


async def test_cloud_layers_fail_clearly_until_fleet_builds_them(fleet):  # noqa: F811
    with pytest.raises(Exception, match="not deployed yet"):
        await Sandbox.create(Image.from_registry(IMAGE).pip_install("mcp"), local=False)


def test_local_runc_opt_in_reaches_the_sdk(monkeypatch, tmp_path):
    """runtime="runc" is the explicit opt-in a local sidecar group needs."""
    pytest.importorskip("cua")
    from cua_sandbox.runtime import native as native_rt

    seen: list = []

    class _Sandboxes:
        async def create(self, options):
            seen.append(options)
            raise _Captured()

    class _Cua:
        def sandboxes(self):
            return _Sandboxes()

    monkeypatch.setattr(native_rt, "local_runtime", lambda: _Cua())
    monkeypatch.setattr(sandbox_module, "_TELEMETRY_AVAILABLE", False)
    img = Image.from_registry("python:3.12-slim", kind="container")
    with pytest.raises(_Captured):
        asyncio.run(Sandbox.create(img, local=True, runtime="runc", sidecars=["redis:7-alpine"]))
    assert seen[0].runtime == "runc"
    assert seen[0].kind == "container"
    assert [s.image for s in seen[0].sidecars] == ["redis:7-alpine"]
    seen.clear()
    with pytest.raises(_Captured):
        asyncio.run(Sandbox.create(img, local=True, sidecars=["redis:7-alpine"]))
    assert seen[0].runtime is None, "no silent choice: the SDK decides and refuses"

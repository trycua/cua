"""The shared sandbox model in cua-sandbox (hermetic).

``Pool.apply(name, SandboxSpec, PoolOptions)`` is the only pool writer: it
goes through the native SDK's ``Fleet.apply``. The pure tests capture what
cua-sandbox hands the SDK; the ``fleet`` ones run the native SDK against the
loopback fake Fleet (``cua-test-fixtures``).
"""

from __future__ import annotations

import warnings
from datetime import timedelta
from types import SimpleNamespace

import pytest
from cua_sandbox import (
    ClaimSecretsNotDelivered,
    CloudOptions,
    Image,
    InvalidArgument,
    Pool,
    PoolAccessDeniedError,
    PoolOptions,
    PoolSpecMismatch,
    Sandbox,
    SandboxSpec,
    WarmPoolAutoscaling,
    generate_claim_token,
)
from cua_sandbox import pool as pool_module
from cua_sandbox import (
    tcp,
)
from cua_sandbox.spec import translate_native_error

from tests._native_fleet import fleet  # noqa: F401 - fixture

ROOTFS = "ghcr.io/trycua/cua-e2e-plain:1"


def _native():
    from cua_sandbox._sdk import native

    return native()


# ── pure ─────────────────────────────────────────────────────────────────


def test_spec_and_options_convert_to_the_native_records():
    spec = SandboxSpec(
        image=Image.from_registry(ROOTFS, kind="container"),
        command=["python", "-m", "srv"],
        env={"A": "1"},
        services={"mcp": 8765},
        wait_for=tcp("mcp"),
        cpu=2,
        memory="2GB",
        claim_secrets=True,
    ).native()
    assert spec.image == ROOTFS
    assert spec.command == ["python", "-m", "srv"]
    assert spec.services == {"mcp": 8765}
    assert spec.readiness.service == "mcp"
    assert spec.memory_mb == 2048
    assert spec.claim_secrets is True
    options = PoolOptions(
        runtime="gvisor",
        warm=True,
        max_pool_size=3,
        idle_ttl=timedelta(hours=1),
        ttl_policy="Cascade",
        claim_ttl=600,
    ).native()
    assert options.runtime == "gvisor"
    assert options.warm is True
    assert options.idle_ttl_seconds == 3600
    assert options.ttl_policy == "Cascade"
    assert options.claim_ttl_seconds == 600
    with pytest.raises(ValueError, match="runtime"):
        PoolOptions(runtime="firecracker").native()
    with pytest.raises(ValueError, match="idle_ttl"):
        PoolOptions(idle_ttl=-1).native()
    # A Windows image boots UEFI; an Image's secret becomes the pool's.
    assert SandboxSpec(image=Image.windows()).native().efi is True


def test_native_errors_map_to_typed_exceptions():
    n = _native()
    mismatch = translate_native_error(n.CuaError.PoolSpecMismatch("pool x: command differs"))
    assert isinstance(mismatch, PoolSpecMismatch)
    assert isinstance(mismatch, InvalidArgument) and isinstance(mismatch, ValueError)
    assert "command differs" in str(mismatch)
    lost = translate_native_error(n.CuaError.ClaimSecretsNotDelivered("claim c released"))
    assert isinstance(lost, ClaimSecretsNotDelivered) and isinstance(lost, TimeoutError)
    other = n.CuaError.Timeout("x")
    assert translate_native_error(other) is other
    assert len(generate_claim_token()) == 64


class _Resources:
    """The Python Fleet client reads the written pool back."""

    def __init__(self, name: str) -> None:
        self.pool = SimpleNamespace(metadata=SimpleNamespace(name=name, namespace=name))
        self.template = SimpleNamespace(metadata=SimpleNamespace(name=name, namespace=name))

    async def get_pool(self, name):
        return self.pool

    async def get_template(self, namespace, name):
        return self.template

    async def close(self):
        pass


def _capture(monkeypatch, name="workspace"):
    seen = []

    async def native_apply(n, spec, options):
        seen.append((n, spec, options))

    monkeypatch.setattr(pool_module, "_native_apply", native_apply)
    monkeypatch.setattr(pool_module, "_FleetClient", lambda: _Resources(name))
    return seen


async def test_pool_apply_writes_through_the_native_fleet_apply(monkeypatch):
    seen = _capture(monkeypatch)
    image = Image.from_registry(ROOTFS, kind="container")
    pool = await Pool.apply(
        "workspace",
        SandboxSpec(image=image, command=["srv"], services={"mcp": 8765}),
        PoolOptions(warm=True),
    )
    assert pool.name == "workspace"
    assert pool._owned_template is not None
    [(name, spec, options)] = seen
    assert name == "workspace"
    assert spec.command == ["srv"]
    # The runtime is resolved (the one rule) before anything is written.
    assert options.runtime == "gvisor"
    assert options.warm is True
    with pytest.raises(ValueError, match="globally unique"):
        await Pool.apply("", SandboxSpec(image=image))
    with pytest.raises(TypeError):
        await Pool.apply("workspace", {"image": ROOTFS})


async def test_the_old_signature_is_a_deprecated_wrapper(monkeypatch):
    seen = _capture(monkeypatch)
    image = Image.from_registry(ROOTFS, kind="container")
    with pytest.warns(DeprecationWarning, match="SandboxSpec"):
        await Pool.apply(
            image,
            name="workspace",
            cpu=4,
            memory_mb=4096,
            autoscaling=WarmPoolAutoscaling(min_pool_size=1, initial_pool_size=2, max_pool_size=5),
            ttl_seconds_after_created=86400,
        )
    [(_, spec, options)] = seen
    assert (spec.image, spec.cpu, spec.memory_mb) == (ROOTFS, 4, 4096)
    assert spec.services == {"env": 3211}, "daemon-agnostic default services"
    assert spec.readiness is None
    assert (options.min_pool_size, options.replicas, options.max_pool_size) == (1, 2, 5)
    assert options.pool_ttl_seconds == 86400
    assert options.runtime == "gvisor"

    seen.clear()
    with warnings.catch_warnings():
        warnings.simplefilter("ignore", DeprecationWarning)
        await Pool.apply(image, name="workspace", replicas=3, services={"server": 8000})
    [(_, spec, options)] = seen
    assert spec.readiness.service == "server", "a server service is the readiness probe"
    assert options.replicas == 3 and options.min_pool_size is None
    with warnings.catch_warnings():
        warnings.simplefilter("ignore", DeprecationWarning)
        with pytest.raises(TypeError):
            await Pool.apply(image)
        with pytest.raises(ValueError, match="globally unique"):
            await Pool.apply(image, name="")
        with pytest.raises(ValueError, match="ttl_seconds_after_created"):
            await Pool.apply(image, name="workspace", ttl_seconds_after_created=-1)


async def test_a_taken_pool_name_is_access_denied(monkeypatch):
    n = _native()

    class _Fleet:
        async def apply(self, name, spec, options):
            raise n.CuaError.Fleet(
                "Fleet denied create pool on pool namespace 'workspace' (HTTP 403). Pool "
                "names are globally unique across accounts"
            )

    monkeypatch.setattr(pool_module, "native_fleet", lambda: _Fleet())
    with pytest.raises(PoolAccessDeniedError, match="globally unique"):
        await Pool.apply(
            "workspace",
            SandboxSpec(image=Image.from_registry(ROOTFS, kind="container")),
        )


async def test_apply_true_needs_a_pool():
    with pytest.raises(ValueError, match="pass pool="):
        await Sandbox.create(
            Image.from_registry(ROOTFS, kind="container"),
            cloud=CloudOptions(apply=True),
        )


# ── the fake Fleet ───────────────────────────────────────────────────────


async def _apply(name: str) -> Pool:
    return await Pool.apply(
        name,
        SandboxSpec(
            image=Image.from_registry(ROOTFS, kind="container"),
            command=["python", "-m", "srv"],
            services={"mcp": 8765},
            cpu=2,
            memory_mb=2048,
        ),
        PoolOptions(warm=True, idle_ttl=3600, ttl_policy="Cascade"),
    )


async def test_apply_export_and_terraform_against_the_fake_fleet(fleet):  # noqa: F811
    pool = await _apply("cua-e2e-py-spec")
    assert pool.name == "cua-e2e-py-spec"
    exported = await Pool.export("cua-e2e-py-spec")
    assert exported.runtime == "gvisor"
    assert exported.spec.command == ["python", "-m", "srv"]
    assert exported.options.warm is True
    assert exported.options.idle_ttl_seconds == 3600
    assert exported.options.ttl_policy == "Cascade"
    hcl = await Pool.export("cua-e2e-py-spec", terraform=True)
    for want in (
        'resource "fleets_pool" "cua_e2e_py_spec" {',
        "  cpu_cores = 2",
        '  memory = "2048Mi"',
        '  runtime = "gvisor"',
        '  command = ["python", "-m", "srv"]',
        "  idle_ttl_seconds = 3600",
        "    min_pool_size = 1",
    ):
        assert want in hcl, (want, hcl)
    await pool.delete()


async def test_a_named_pool_is_compared_and_applied(fleet):  # noqa: F811
    await _apply("cua-e2e-py-mm")
    await Pool.check("cua-e2e-py-mm", SandboxSpec(command=["python", "-m", "srv"]))
    with pytest.raises(PoolSpecMismatch) as error:
        await Pool.check("cua-e2e-py-mm", SandboxSpec(command=["node", "srv.js"], cpu=4))
    message = str(error.value)
    assert "command: pool has" in message and "cpu: pool has 2, requested 4" in message
    assert "apply=True" in message

    # Sandbox.create with the named pool: a mismatch fails before any claim.
    with pytest.raises(PoolSpecMismatch, match="command"):
        await Sandbox.create(
            command=["node", "srv.js"],
            cloud=CloudOptions(pool="cua-e2e-py-mm"),
        )
    client = fleet()
    try:
        assert await client.list_claims("cua-e2e-py-mm") == [], "nothing claimed"
    finally:
        await client.close()

    # apply=True reconciles the template's given fields; the rest is kept.
    await Pool.apply_template("cua-e2e-py-mm", SandboxSpec(command=["node", "srv.js"]))
    await Pool.check("cua-e2e-py-mm", SandboxSpec(command=["node", "srv.js"]))
    exported = await Pool.export("cua-e2e-py-mm")
    assert exported.spec.command == ["node", "srv.js"]
    assert exported.spec.services == {"mcp": 8765}
    assert exported.spec.cpu == 2

    # Managed pools refuse apply=True (their template is their key).
    with pytest.raises(ValueError, match="managed pool"):
        await Sandbox.create(
            command=["x"],
            cloud=CloudOptions(pool="cua-auto-abc", apply=True),
        )


def test_macos_runtime_is_refused_before_any_fleet_call():
    with pytest.raises(ValueError, match="Fleet does not offer macOS sandboxes"):
        PoolOptions(runtime="macos").native()
    with pytest.raises(ValueError, match="runtime must be"):
        PoolOptions(runtime="firecracker").native()

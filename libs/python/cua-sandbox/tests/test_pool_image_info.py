"""``Sandbox.image_info`` for claims on a named Fleet pool: its template's
image, from the native ``Fleet.pool_image_info``.

Hermetic: the loopback fake Fleet (``cua-test-fixtures``) and fake SDK
records; registry reads are off (conftest), so only a digest-pinned template
carries a digest here (the Rust tests cover tag resolution).
"""

from __future__ import annotations

from types import SimpleNamespace

from cua_sandbox import (
    CloudOptions,
    Image,
    Pool,
    PoolOptions,
    Sandbox,
    SandboxSpec,
    _sdk,
)
from cua_sandbox import pool as pool_module
from cua_sandbox.transport.env import EnvTransport

from tests._native_fleet import fleet  # noqa: F401 - fixture

DIGEST = "sha256:" + "c" * 64
PINNED = f"ghcr.io/trycua/cua-e2e-plain@{DIGEST}"


def _record(**overrides) -> SimpleNamespace:
    fields = dict(
        reference=PINNED,
        pinned_ref=PINNED,
        digest=DIGEST,
        variant="rootfs",
        arch="amd64",
        os="linux",
        emulated=False,
    )
    fields.update(overrides)
    return SimpleNamespace(**fields)


async def test_a_named_pool_claim_records_the_template_image(monkeypatch):
    async def pool_image_info(pool):
        assert pool == "cua-e2e-named"
        return _record()

    monkeypatch.setattr(_sdk, "pool_image_info", pool_image_info)
    sb = Sandbox(EnvTransport(url="http://127.0.0.1:1"), name="sb")
    await pool_module._record_pool_image(sb, "cua-e2e-named")
    assert sb.image_info.pinned_ref == PINNED
    assert sb.image_info.digest == DIGEST


async def test_an_unreadable_pool_leaves_image_info_unset(monkeypatch):
    async def fails(pool):
        raise RuntimeError("boom")

    monkeypatch.setattr(_sdk, "pool_image_info", fails)
    sb = Sandbox(EnvTransport(url="http://127.0.0.1:1"), name="sb")
    await pool_module._record_pool_image(sb, "cua-e2e-named")
    assert sb.image_info is None


async def test_the_native_fleet_reports_a_named_pool_template_image(fleet):  # noqa: F811
    await Pool.apply(
        "cua-e2e-py-digest",
        SandboxSpec(image=Image.from_registry(PINNED, kind="container")),
        PoolOptions(runtime="gvisor"),
    )
    record = await _sdk.pool_image_info("cua-e2e-py-digest")
    assert record is not None
    assert record.pinned_ref == PINNED
    assert record.digest == DIGEST
    assert record.variant == "rootfs"
    # A pool that does not exist: None, never an error.
    assert await _sdk.pool_image_info("cua-e2e-py-missing") is None


async def test_a_claim_on_a_named_pool_reports_its_image(fleet):  # noqa: F811
    await Pool.apply(
        "cua-e2e-py-claim",
        SandboxSpec(image=Image.from_registry(PINNED, kind="container")),
        PoolOptions(runtime="gvisor"),
    )
    async with Sandbox.ephemeral(
        None, cloud=CloudOptions(pool="cua-e2e-py-claim"), local=False
    ) as sb:
        assert sb.image_info is not None
        assert sb.image_info.pinned_ref == PINNED
        assert sb.image_info.digest == DIGEST
        # The SDK handle opened for the claim reports the same.
        handle = await sb._transport._fleet_handle()
        assert handle.image_info().pinned_ref == PINNED

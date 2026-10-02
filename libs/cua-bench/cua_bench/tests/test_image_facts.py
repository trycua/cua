"""``image_facts``: what ran, recorded per result (kind, runtime, image ref,
variant, digest, arch). Hermetic: fake sandboxes and a cua-sandbox Sandbox
with a faked SDK answer; no network, container, VM or Fleet.
"""

from __future__ import annotations

from types import SimpleNamespace

from cua_bench.sandboxes import image_facts
from cua_bench.targets import EnvSpec

DIGEST = "sha256:" + "d" * 64
PINNED = f"ghcr.io/trycua/linux@{DIGEST}"


def _info(**overrides) -> SimpleNamespace:
    fields = dict(
        reference="ghcr.io/trycua/linux:24.04",
        pinned_ref=PINNED,
        digest=DIGEST,
        variant="rootfs",
        arch="amd64",
        os="linux",
        emulated=False,
    )
    fields.update(overrides)
    return SimpleNamespace(**fields)


def _pool_spec() -> EnvSpec:
    # `--image pool:<name>`: no image on the spec; the pool's template has it.
    return EnvSpec(provider="native", pool="cua-e2e-named")


def test_a_named_pool_claim_records_the_template_digest():
    facts = image_facts(SimpleNamespace(image_info=_info()), _pool_spec())
    assert facts == {
        "kind": "container",
        "runtime": None,
        "image_ref": "ghcr.io/trycua/linux:24.04",
        "image_variant": "rootfs",
        "image_digest": PINNED,
        "arch": "amd64",
    }


def test_an_unresolved_template_records_the_reference_without_a_digest():
    info = _info(reference="registry.example.com/team/private:1", pinned_ref="", digest="")
    facts = image_facts(SimpleNamespace(image_info=info), _pool_spec())
    assert facts["image_ref"] == "registry.example.com/team/private:1"
    assert facts["image_digest"] is None


def test_no_image_info_keeps_the_spec():
    class Raising:
        @property
        def image_info(self):
            raise RuntimeError("boom")

    spec = EnvSpec(provider="native", image="ghcr.io/trycua/linux:24.04")
    for sandbox in (SimpleNamespace(image_info=None), Raising(), object()):
        facts = image_facts(sandbox, spec)
        assert facts["image_ref"] == "ghcr.io/trycua/linux:24.04"
        assert facts["image_digest"] is None and facts["arch"] is None


async def test_a_cua_sandbox_named_pool_claim_feeds_image_facts(monkeypatch):
    """End to end through cua-sandbox's claim hook with a faked SDK answer."""
    from cua_sandbox import Sandbox, _sdk
    from cua_sandbox import pool as pool_module
    from cua_sandbox.transport.env import EnvTransport

    async def pool_image_info(pool):
        assert pool == "cua-e2e-named"
        return _info()

    monkeypatch.setattr(_sdk, "pool_image_info", pool_image_info)
    sandbox = Sandbox(EnvTransport(url="http://127.0.0.1:1"), name="sb")
    await pool_module._record_pool_image(sandbox, "cua-e2e-named")
    facts = image_facts(sandbox, _pool_spec())
    assert facts["image_digest"] == PINNED
    assert facts["arch"] == "amd64"

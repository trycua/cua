"""``image_facts``: what ran, recorded per result (kind, runtime, image ref,
variant, digest, arch). Hermetic: fake sandboxes; no network, container or VM.
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


def _bare_spec() -> EnvSpec:
    # No image on the spec (the built-in image): the SDK reports what ran.
    return EnvSpec(provider="native")


def test_the_sdk_reported_image_is_recorded_with_its_digest():
    facts = image_facts(SimpleNamespace(image_info=_info()), _bare_spec())
    assert facts == {
        "kind": "container",
        "runtime": None,
        "image_ref": "ghcr.io/trycua/linux:24.04",
        "image_variant": "rootfs",
        "image_digest": PINNED,
        "arch": "amd64",
    }


def test_an_unresolved_image_records_the_reference_without_a_digest():
    info = _info(reference="registry.example.com/team/private:1", pinned_ref="", digest="")
    facts = image_facts(SimpleNamespace(image_info=info), _bare_spec())
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

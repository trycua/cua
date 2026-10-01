"""The Fleet runtime/image rule, through the cua SDK binding.

cua-sandbox does not re-implement the rule: it calls the SDK
(``fleet_resolve_runtime``). These tests run the shared vectors
(``libs/cua/crates/cua-fleet/tests/fixtures/image-variants.json``, also run
by cua-fleet's Rust tests) through the pure bindings, so both languages
agree on what a manifest means. No registry is read.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest
from cua_sandbox._sdk import native

from tests import _image_fixtures

VECTORS = (
    Path(__file__).resolve().parents[3]
    / "cua"
    / "crates"
    / "cua-fleet"
    / "tests"
    / "fixtures"
    / "image-variants.json"
)
CASES = json.loads(VECTORS.read_text())["cases"]


@pytest.mark.parametrize("case", CASES, ids=[c["name"] for c in CASES])
def test_shared_vectors_classify_and_pick_the_runtime(case):
    n = native()
    config = None if case["config"] is None else json.dumps(case["config"])
    if "refused" in case:
        with pytest.raises(n.CuaError.Unsupported, match=case["refused"]):
            n.fleet_image_variant(json.dumps(case["manifest"]), config)
        return
    variant = n.fleet_image_variant(json.dumps(case["manifest"]), config)
    assert variant == case["variant"]
    runtimes = case["runtimes"]
    assert n.fleet_check_runtime(None, case["reference"], variant) == runtimes["default"]
    for runtime in ("kubevirt", "gvisor"):
        if runtimes[runtime]:
            assert n.fleet_check_runtime(runtime, case["reference"], variant) == runtime
        else:
            with pytest.raises(n.CuaError.InvalidArgument):
                n.fleet_check_runtime(runtime, case["reference"], variant)


FALLBACKS = json.loads(VECTORS.read_text())["fallbacks"]


@pytest.mark.parametrize("case", FALLBACKS, ids=[c["reference"] for c in FALLBACKS])
def test_shared_fallback_vectors_pick_the_runtime_from_the_reference(case):
    # The pure rule with no variant: the manifest could not be read.
    n = native()
    assert n.fleet_check_runtime(None, case["reference"], None) == case["runtime"]


def test_an_unreadable_manifest_falls_back_to_the_reference():
    # The real binding; conftest turns registry reads off
    # (CUA_FLEET_IMAGE_INSPECT=0), so every manifest is unreadable. No network.
    resolve = _image_fixtures.REAL
    image = "ghcr.io/trycua/cua-desktop-linux:docker-latest"
    # An explicit runtime is sent unchecked.
    assert resolve("kubevirt", image) == "kubevirt"
    assert resolve("gvisor", image) == "gvisor"
    # An unset one is guessed from the reference, never refused.
    assert resolve(None, image) == "gvisor"
    assert resolve(None, "ghcr.io/trycua/cua-desktop-linux:latest") == "kubevirt"

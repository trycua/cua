from __future__ import annotations

import tomllib
from pathlib import Path


def test_metadata_pins_published_cua_train() -> None:
    project = tomllib.loads((Path(__file__).parents[1] / "pyproject.toml").read_text())["project"]
    assert project["version"] == "0.0.10"
    assert project["dependencies"] == ["cua-train==0.1.6"]


def test_publish_prepare_checks_out_canonical_metadata() -> None:
    workflow = Path(__file__).parents[3] / ".github/workflows/cua-fleet-publish.yml"
    prepare, _ = workflow.read_text().split("\n  repackage:", 1)
    assert prepare.count("    steps:\n") == 1
    assert (
        "    steps:\n"
        "      - uses: actions/checkout@34e114876b0b11c390a56381ad16ebd13914f8d5 # v4\n"
        "      - name: Determine and validate patch version"
    ) in prepare
    assert "@v" not in workflow.read_text()


def test_publish_repackages_the_published_cua_train_0_1_5_wheels() -> None:
    workflow = (Path(__file__).parents[3] / ".github/workflows/cua-fleet-publish.yml").read_text()

    expected_sources = {
        "cua_train-0.1.6-py3-none-manylinux_2_34_x86_64.whl": "418db974949f4020fae2f3ed2f5a8d0db6ddce38e34f5db89283ac479d816a55",
        "cua_train-0.1.6-py3-none-manylinux_2_34_aarch64.whl": "80526bf94ed535383b3dddd9a7da800da9fcb32837c21c2e2caa39390e3548c9",
        "cua_train-0.1.6-py3-none-macosx_10_12_x86_64.whl": "87cb4220054514ac5f44118d490ae404fbca5c97d2b964cd4fcaf1facd391fc0",
        "cua_train-0.1.6-py3-none-macosx_11_0_arm64.whl": "17563c0504c9270e570332924f3092faec3dcdbf48557a70ee830e165ac7751c",
    }

    for wheel, digest in expected_sources.items():
        assert f"wheel: {wheel}\n            sha256: {digest}" in workflow

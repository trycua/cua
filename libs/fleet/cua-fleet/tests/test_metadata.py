from __future__ import annotations

import tomllib
from pathlib import Path


def test_metadata_pins_published_cua_train() -> None:
    project = tomllib.loads((Path(__file__).parents[1] / "pyproject.toml").read_text())["project"]
    assert project["version"] == "0.0.8"
    assert project["dependencies"] == ["cua-train==0.1.4"]


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


def test_publish_repackages_the_published_cua_train_0_1_4_wheels() -> None:
    workflow = (Path(__file__).parents[3] / ".github/workflows/cua-fleet-publish.yml").read_text()

    expected_sources = {
        "cua_train-0.1.4-py3-none-manylinux_2_34_x86_64.whl": "8b2c20a603772408ad25629e5fa16bfc36bb81ed6a876fd99f1ccfab5c242252",
        "cua_train-0.1.4-py3-none-manylinux_2_34_aarch64.whl": "d0ea3e67a67c394ca05fad227ddd53987b53fbfbff907e43aa2ac6476f941df4",
        "cua_train-0.1.4-py3-none-macosx_10_12_x86_64.whl": "20e59a46ae5bc20bc48e34bc8b0c8b623682068be5c31b430477bdf57e946745",
        "cua_train-0.1.4-py3-none-macosx_11_0_arm64.whl": "cf5f4e26f77b4438849b5e6216829422980150439a88aa9a62a78dab74a95eb6",
    }

    for wheel, digest in expected_sources.items():
        assert f"wheel: {wheel}\n            sha256: {digest}" in workflow

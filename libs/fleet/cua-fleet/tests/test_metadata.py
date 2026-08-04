from __future__ import annotations

import tomllib
from pathlib import Path


def test_metadata_pins_published_cua_train() -> None:
    project = tomllib.loads((Path(__file__).parents[1] / "pyproject.toml").read_text())["project"]
    assert project["version"] == "0.0.9"
    assert project["dependencies"] == ["cua-train==0.1.5"]


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
        "cua_train-0.1.5-py3-none-manylinux_2_34_x86_64.whl": "72824b317888d1bbf21585907c8291ce648007e052e92c1e4cd8d475750b57a5",
        "cua_train-0.1.5-py3-none-manylinux_2_34_aarch64.whl": "d6adca168fbd9777ef3fccf21f0d39b180cde440494f8f0ff7d47a46de2e6d5a",
        "cua_train-0.1.5-py3-none-macosx_10_12_x86_64.whl": "605cdd794edbd04a88f5a99f3713fd5b87fa8c634285839015a61894544cd1d1",
        "cua_train-0.1.5-py3-none-macosx_11_0_arm64.whl": "82ec2795fad892dc6238fcbcabb861e78d214f6603d765419a2b10fddef05fda",
    }

    for wheel, digest in expected_sources.items():
        assert f"wheel: {wheel}\n            sha256: {digest}" in workflow

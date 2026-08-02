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

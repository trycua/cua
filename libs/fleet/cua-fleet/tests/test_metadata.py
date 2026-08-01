from __future__ import annotations

import tomllib
from pathlib import Path


def test_metadata_pins_published_cua_train() -> None:
    project = tomllib.loads((Path(__file__).parents[1] / "pyproject.toml").read_text())["project"]
    assert project["version"] == "0.0.7"
    assert project["dependencies"] == ["cua-train==0.1.3"]

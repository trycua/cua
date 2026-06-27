from __future__ import annotations

import importlib
import pathlib
import tomllib


def test_placeholder_package_metadata() -> None:
    repo_root = pathlib.Path(__file__).resolve().parents[4]
    pyproject_path = repo_root / "libs/python/cua-cloud/pyproject.toml"

    package = importlib.import_module("cua_cloud")

    assert package.__version__ == "0.1.0"

    project = tomllib.loads(pyproject_path.read_text())["project"]
    assert project["name"] == "cua-cloud"
    assert project["description"] == "Placeholder package for future Cua Cloud Python SDK."

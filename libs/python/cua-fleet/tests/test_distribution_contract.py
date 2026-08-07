from __future__ import annotations

from pathlib import Path
import tomllib


PACKAGE_ROOT = Path(__file__).resolve().parents[1]


def test_release_metadata_matches_the_uniffi_distribution() -> None:
    project = tomllib.loads((PACKAGE_ROOT / "pyproject.toml").read_text())["project"]

    assert project["name"] == "cua-fleet"
    assert project["version"] == "0.1.7"
    assert project["description"] == "Cua Fleet UniFFI SDK"
    assert project["requires-python"] == ">=3.10"
    assert project.get("dependencies", []) == []


def test_repository_metadata_does_not_build_a_compatibility_package() -> None:
    pyproject = tomllib.loads((PACKAGE_ROOT / "pyproject.toml").read_text())
    readme = (PACKAGE_ROOT / "README.md").read_text()

    assert "build-system" not in pyproject
    assert not (PACKAGE_ROOT / "cua_fleet").exists()
    assert "from fleet_sdk import" in readme
    assert "from cua_fleet" not in readme
    assert "from cua_train" not in readme
    assert "TrainClient" not in readme

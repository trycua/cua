from importlib.metadata import distribution
from pathlib import Path

import fleet_sdk


def test_fleet_sdk_is_provided_by_published_fleet_distribution():
    cua_fleet_distribution = distribution("cua-fleet")
    installed_files = {str(path) for path in cua_fleet_distribution.files or []}
    distribution_root = Path(cua_fleet_distribution.locate_file(".")).resolve()
    binding_path = Path(fleet_sdk.__file__).resolve()

    assert cua_fleet_distribution.version == "0.0.7"
    assert "fleet_sdk/__init__.py" in installed_files
    assert binding_path.is_relative_to(distribution_root)

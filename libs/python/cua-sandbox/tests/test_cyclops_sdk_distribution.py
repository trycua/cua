from importlib.metadata import distribution, requires
from pathlib import Path

import cua_fleet
import cua_train
import cyclops_sdk
from cua_sandbox.transport.fleet_cloud import FleetCloudTransport


def test_cyclops_sdk_is_provided_by_published_fleets_transitive_train_distribution():
    cua_fleet_distribution = distribution("cua-fleet")
    cua_train_distribution = distribution("cua-train")
    installed_files = {str(path) for path in cua_train_distribution.files or []}
    distribution_root = Path(cua_train_distribution.locate_file(".")).resolve()
    binding_path = Path(cyclops_sdk.__file__).resolve()

    assert cua_fleet_distribution.version == "0.0.4"
    assert "cua-train==0.1.1" in (requires("cua-fleet") or [])
    assert cua_fleet.TrainClient is cua_train.TrainClient
    assert cua_fleet.__all__ == ["TrainClient"]
    assert "cyclops_sdk/__init__.py" in installed_files
    assert binding_path.is_relative_to(distribution_root)
    assert FleetCloudTransport.__module__ == "cua_sandbox.transport.fleet_cloud"

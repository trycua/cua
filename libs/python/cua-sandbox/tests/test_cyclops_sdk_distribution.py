from importlib.metadata import distribution
from pathlib import Path

import cyclops_sdk
from cua_sandbox.transport.fleet_cloud import FleetCloudTransport


def test_cyclops_sdk_is_provided_by_cua_train_distribution():
    cua_train = distribution("cua-train")
    installed_files = {str(path) for path in cua_train.files or []}
    distribution_root = Path(cua_train.locate_file(".")).resolve()
    binding_path = Path(cyclops_sdk.__file__).resolve()

    assert "cyclops_sdk/__init__.py" in installed_files
    assert binding_path.is_relative_to(distribution_root)
    assert FleetCloudTransport.__module__ == "cua_sandbox.transport.fleet_cloud"

from __future__ import annotations

import importlib
import sys
import tomllib
from pathlib import Path

PACKAGE_ROOT = Path(__file__).resolve().parents[1]
REPOSITORY_ROOT = PACKAGE_ROOT.parents[2]
TRAIN_SOURCE_ROOT = REPOSITORY_ROOT / "libs/python/cua-train/src"


def test_package_metadata_pins_binding_bearing_cua_train() -> None:
    project = tomllib.loads((PACKAGE_ROOT / "pyproject.toml").read_text())["project"]

    assert project["requires-python"] == ">=3.10"
    assert project["dependencies"] == ["cua-train==0.1.1"]


def test_cua_fleet_reexports_train_client(monkeypatch) -> None:
    monkeypatch.syspath_prepend(str(TRAIN_SOURCE_ROOT))
    monkeypatch.syspath_prepend(str(PACKAGE_ROOT))
    sys.modules.pop("cua_fleet", None)
    sys.modules.pop("cua_train", None)

    cua_fleet = importlib.import_module("cua_fleet")
    cua_train = importlib.import_module("cua_train")

    assert cua_fleet.TrainClient is cua_train.TrainClient
    assert cua_fleet.__all__ == ["TrainClient"]

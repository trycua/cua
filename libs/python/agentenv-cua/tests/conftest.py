"""Loading the macOS drivers' and apps' folders the way a computer does: each is a folder with its own ``driver.py`` or
``app.py`` and sibling modules, installed on its own, so the tests load each from its folder and forget its modules."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

PACKAGE = Path(__file__).parents[1] / "src" / "agentenv_cua"
DRIVERS = PACKAGE / "macos-drivers" / "envs"
APPS = PACKAGE / "macos-apps" / "envs"
ARTIFACTS = PACKAGE / "macos-apps" / "artifacts"
#: The module names every folder has its own copy of.
SIBLINGS = ("driver", "macos_computer", "app", "bench_macos", "lab", "sentinel", "pin_manifest")


def load(folder: Path, filename: str = "driver.py"):
    """The folder's module, imported with the folder first on ``sys.path`` and no sibling modules left over from
    another folder."""
    for name in SIBLINGS:
        sys.modules.pop(name, None)
    sys.path.insert(0, str(folder))
    try:
        spec = importlib.util.spec_from_file_location(
            f"_loaded_{folder.name}_{filename[:-3]}", folder / filename
        )
        module = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = module
        spec.loader.exec_module(module)
        return module
    finally:
        sys.path.remove(str(folder))


@pytest.fixture(scope="module")
def agent_desktop():
    return load(DRIVERS / "agent-desktop")


@pytest.fixture(scope="module")
def harness():
    return load(DRIVERS / "macos-harness")


@pytest.fixture(scope="module")
def codex():
    return load(DRIVERS / "codex-cu")

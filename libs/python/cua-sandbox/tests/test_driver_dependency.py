"""Keep the native Driver an explicit install option with a compatible range.

An exact pin broke `pip install cua-sandbox[driver]` once the pinned Driver release
was removed from PyPI, so the extra is a range."""

import re
import tomllib
from pathlib import Path


def test_driver_extra_is_a_range_and_optional():
    package = Path(__file__).resolve().parents[1]
    sandbox = tomllib.loads((package / "pyproject.toml").read_text())["project"]

    # The upper bound moves with each Driver minor (release-please.yml runs
    # .github/scripts/sync_cua_sandbox_driver_bound.py), so only the shape is fixed.
    (driver,) = sandbox["optional-dependencies"]["driver"]
    assert re.fullmatch(r"cua-driver>=0\.37\.0,<0\.\d+", driver), driver
    assert not any("==" in item for item in sandbox["optional-dependencies"]["driver"])
    assert not any(item.startswith("cua-driver") for item in sandbox["dependencies"])

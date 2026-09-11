"""Keep the native Driver an explicit, version-matched install option."""

import tomllib
from pathlib import Path


def test_driver_extra_is_exact_and_optional():
    package = Path(__file__).resolve().parents[1]
    sandbox = tomllib.loads((package / "pyproject.toml").read_text())["project"]

    assert sandbox["optional-dependencies"]["driver"] == ["cua-driver==0.26.0"]
    assert not any(item.startswith("cua-driver") for item in sandbox["dependencies"])

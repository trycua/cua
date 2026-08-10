"""Regression tests for cua-sandbox release-bump wiring."""

import configparser
from pathlib import Path
import tomllib
import unittest


REPO_ROOT = Path(__file__).resolve().parents[3]
SANDBOX_ROOT = REPO_ROOT / "libs/python/cua-sandbox"


class TestCuaSandboxReleaseWiring(unittest.TestCase):
    """Keep the legacy bump configuration synchronized with package metadata."""

    def test_bump_config_tracks_the_current_package_version(self) -> None:
        config = configparser.ConfigParser()
        config.read(SANDBOX_ROOT / ".bumpversion.cfg")
        with (SANDBOX_ROOT / "pyproject.toml").open("rb") as pyproject_file:
            project = tomllib.load(pyproject_file)["project"]

        self.assertEqual(config["bumpversion"]["current_version"], project["version"])


if __name__ == "__main__":
    unittest.main()

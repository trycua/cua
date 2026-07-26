"""Tests for targeted Release Please request resolution."""

import importlib.util
from pathlib import Path
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "resolve_release_please_request.py"
SPEC = importlib.util.spec_from_file_location("resolve_release_please_request", SCRIPT)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class TestResolveReleasePleaseRequest(unittest.TestCase):
    def setUp(self) -> None:
        self.manifest = {
            "libs/cua-driver": "0.9.0",
            "libs/lume": "0.4.7",
        }

    def test_resolves_each_component_path(self) -> None:
        driver = MODULE.resolve_request(self.manifest, "cua-driver-rs", "automatic")
        lume = MODULE.resolve_request(self.manifest, "lume", "automatic")

        self.assertEqual(driver["path"], "libs/cua-driver")
        self.assertEqual(lume["path"], "libs/lume")
        self.assertIsNone(driver["release_as"])

    def test_calculates_patch_minor_and_major_versions(self) -> None:
        self.assertEqual(MODULE.bump_version("0.9.0", "patch"), "0.9.1")
        self.assertEqual(MODULE.bump_version("0.9.0", "minor"), "0.10.0")
        self.assertEqual(MODULE.bump_version("0.9.0", "major"), "1.0.0")

    def test_rejects_unknown_components_and_bumps(self) -> None:
        with self.assertRaisesRegex(ValueError, "unsupported component"):
            MODULE.resolve_request(self.manifest, "other", "patch")
        with self.assertRaisesRegex(ValueError, "unsupported bump type"):
            MODULE.resolve_request(self.manifest, "lume", "other")

    def test_rejects_missing_or_non_stable_manifest_versions(self) -> None:
        with self.assertRaisesRegex(ValueError, "does not contain"):
            MODULE.resolve_request({}, "lume", "patch")
        with self.assertRaisesRegex(ValueError, "not stable SemVer"):
            MODULE.bump_version("1.0.0-rc.1", "patch")


if __name__ == "__main__":
    unittest.main()

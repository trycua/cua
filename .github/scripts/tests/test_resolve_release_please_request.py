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
            "libs/cua-driver/rust/crates/cua-perception": "0.1.0",
            "libs/cua-driver": "0.9.0",
            "libs/lume": "0.4.7",
            "libs/python/cua-sandbox": "0.4.3",
            "libs/cua-spacesd": "0.1.0",
            "libs/cua": "0.2.0",
            "apps/cua-spaces-macos": "0.1.0",
        }

    def test_resolves_each_component_path(self) -> None:
        driver = MODULE.resolve_request(self.manifest, "cua-driver-rs", "automatic")
        lume = MODULE.resolve_request(self.manifest, "lume", "automatic")
        sandbox = MODULE.resolve_request(self.manifest, "sandbox", "automatic")
        perception = MODULE.resolve_request(self.manifest, "cua-perception", "automatic")

        self.assertEqual(driver["path"], "libs/cua-driver")
        self.assertEqual(lume["path"], "libs/lume")
        self.assertEqual(sandbox["path"], "libs/python/cua-sandbox")
        self.assertEqual(
            perception["path"], "libs/cua-driver/rust/crates/cua-perception"
        )
        self.assertEqual(sandbox["current_version"], "0.4.3")
        self.assertIsNone(sandbox["release_as"])
        self.assertIsNone(driver["release_as"])

    def test_resolves_spacesd(self) -> None:
        request = MODULE.resolve_request(self.manifest, "cua-spacesd", "minor")
        self.assertEqual(request["path"], "libs/cua-spacesd")
        self.assertEqual(request["release_as"], "0.2.0")

    def test_resolves_cua_sdk(self) -> None:
        request = MODULE.resolve_request(self.manifest, "cua-sdk", "patch")
        self.assertEqual(request["path"], "libs/cua")
        self.assertEqual(request["release_as"], "0.2.1")

    def test_resolves_cua_spaces_to_the_macos_app(self) -> None:
        request = MODULE.resolve_request(self.manifest, "cua-spaces", "patch")
        self.assertEqual(request["path"], "apps/cua-spaces-macos")
        self.assertEqual(request["current_version"], "0.1.0")
        self.assertEqual(request["release_as"], "0.1.1")

    def test_calculates_patch_minor_and_major_versions(self) -> None:
        self.assertEqual(MODULE.bump_version("0.9.0", "patch"), "0.9.1")
        self.assertEqual(MODULE.bump_version("0.9.0", "minor"), "0.10.0")
        self.assertEqual(MODULE.bump_version("0.9.0", "major"), "1.0.0")
        self.assertEqual(
            MODULE.resolve_request(self.manifest, "sandbox", "minor")["release_as"],
            "0.5.0",
        )

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

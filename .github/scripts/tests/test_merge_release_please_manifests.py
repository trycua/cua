"""Tests for reconciling independent Release Please component branches."""

import importlib.util
from pathlib import Path
import unittest


SCRIPT_PATH = (
    Path(__file__).resolve().parents[1] / "merge_release_please_manifests.py"
)
SPEC = importlib.util.spec_from_file_location(
    "merge_release_please_manifests", SCRIPT_PATH
)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class TestMergeReleasePleaseManifests(unittest.TestCase):
    def test_lume_release_keeps_newer_driver_version_from_main(self) -> None:
        main_manifest = {
            "libs/cua-driver": "0.11.0",
            "libs/lume": "0.3.16",
        }
        stale_lume_release_manifest = {
            "libs/cua-driver": "0.10.0",
            "libs/lume": "0.4.0",
        }

        merged = MODULE.merge_component_versions(
            main_manifest,
            stale_lume_release_manifest,
            ["lume"],
        )

        self.assertEqual(
            merged,
            {
                "libs/cua-driver": "0.11.0",
                "libs/lume": "0.4.0",
            },
        )

    def test_driver_release_keeps_newer_lume_version_from_main(self) -> None:
        main_manifest = {
            "libs/cua-driver": "0.10.0",
            "libs/lume": "0.4.0",
        }
        stale_driver_release_manifest = {
            "libs/cua-driver": "0.11.0",
            "libs/lume": "0.3.16",
        }

        merged = MODULE.merge_component_versions(
            main_manifest,
            stale_driver_release_manifest,
            ["cua-driver-rs"],
        )

        self.assertEqual(
            merged,
            {
                "libs/cua-driver": "0.11.0",
                "libs/lume": "0.4.0",
            },
        )

    def test_missing_component_version_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "does not contain 'libs/lume'"):
            MODULE.merge_component_versions(
                {"libs/cua-driver": "0.11.0", "libs/lume": "0.3.16"},
                {"libs/cua-driver": "0.10.0"},
                ["lume"],
            )


if __name__ == "__main__":
    unittest.main()

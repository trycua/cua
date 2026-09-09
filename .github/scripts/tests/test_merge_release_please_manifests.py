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
    def test_sandbox_release_preserves_other_components_and_new_manifest_entries(self) -> None:
        current = {
            "libs/cua-driver": "0.25.0",
            "libs/lume": "0.5.3",
            "libs/python/cua-sandbox": "0.4.3",
            "future/component": "1.0.0",
        }
        stale = {
            "libs/cua-driver": "0.24.0",
            "libs/lume": "0.5.2",
            "libs/python/cua-sandbox": "0.5.0",
        }
        self.assertEqual(
            MODULE.merge_component_versions(current, stale, ["sandbox"]),
            {**current, "libs/python/cua-sandbox": "0.5.0"},
        )

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

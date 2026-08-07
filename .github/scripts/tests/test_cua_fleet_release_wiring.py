"""Regression tests for the canonical cua-fleet PyPI mirror workflow."""

from __future__ import annotations

import json
from pathlib import Path
import re
import tomllib
import unittest


REPO_ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = REPO_ROOT / ".github/workflows/cd-py-fleet.yml"
MANIFEST = REPO_ROOT / "libs/python/cua-fleet/canonical-wheels.json"
PYPROJECT = REPO_ROOT / "libs/python/cua-fleet/pyproject.toml"


class TestCuaFleetReleaseWiring(unittest.TestCase):
    def test_manifest_pins_the_complete_0_1_7_release(self) -> None:
        manifest = json.loads(MANIFEST.read_text())
        project = tomllib.loads(PYPROJECT.read_text())["project"]

        self.assertEqual(manifest["version"], "0.1.7")
        self.assertEqual(project["version"], manifest["version"])
        expected_platforms = {
            "manylinux_2_34_x86_64",
            "manylinux_2_34_aarch64",
            "macosx_10_12_x86_64",
            "macosx_11_0_arm64",
            "win_amd64",
        }
        self.assertEqual(
            set(manifest["wheels"]),
            {
                f"cua_fleet-0.1.7-py3-none-{platform}.whl"
                for platform in expected_platforms
            },
        )
        for digest in manifest["wheels"].values():
            self.assertRegex(digest, re.compile(r"^[0-9a-f]{64}$"))

    def test_workflow_mirrors_canonical_wheels_without_repackaging(self) -> None:
        workflow = WORKFLOW.read_text()

        self.assertIn("libs/python/cua-fleet/canonical-wheels.json", workflow)
        self.assertIn("https://wheels.cua.ai/simple/cua-fleet/$WHEEL", workflow)
        self.assertIn(".github/scripts/verify_cua_fleet_wheels.py", workflow)
        self.assertIn("windows-2022", workflow)
        self.assertIn("ctypes.WinDLL", workflow)
        self.assertIn("vswhere.exe", workflow)
        self.assertIn("$dumpbin =", workflow)
        self.assertNotIn("& dumpbin /DEPENDENTS", workflow)
        self.assertIn("$dumpbin.FullName /DEPENDENTS", workflow)
        self.assertIn("api-ms-win-|ext-ms-win-", workflow)
        self.assertIn("non-system DLL dependency", workflow)
        self.assertIn("win_amd64", workflow)
        self.assertIn("hashlib.sha256", workflow)
        self.assertIn('files+=("dist/$wheel")', workflow)
        self.assertIn('python -m twine upload --verbose "${files[@]}"', workflow)
        self.assertNotIn("repackage_cua_train_wheel.py", workflow)
        self.assertNotIn("cua_train", workflow)
        self.assertNotIn("cua-train", workflow)
        self.assertNotIn("unsupported-windows", workflow)


if __name__ == "__main__":
    unittest.main()

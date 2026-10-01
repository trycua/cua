"""Regression tests for cua-sandbox Release Please ownership."""

import json
from pathlib import Path
import unittest


REPO_ROOT = Path(__file__).resolve().parents[3]
SANDBOX_ROOT = REPO_ROOT / "libs/python/cua-sandbox"


class TestCuaSandboxReleaseWiring(unittest.TestCase):
    """Keep Sandbox version sources owned by one release system."""

    def test_release_please_owns_sandbox_version_sources(self) -> None:
        config = json.loads((REPO_ROOT / "release-please-config.json").read_text())
        package = config["packages"]["libs/python/cua-sandbox"]
        self.assertEqual(package["release-type"], "simple")
        self.assertEqual(package["component"], "sandbox")
        self.assertEqual(package["version-file"], "VERSION")
        self.assertEqual(package["changelog-path"], "CHANGELOG.md")
        self.assertIn(
            {"type": "toml", "path": "pyproject.toml", "jsonpath": "$.project.version"},
            package["extra-files"],
        )
        self.assertIn(
            {"type": "generic", "path": "cua_sandbox/__init__.py"},
            package["extra-files"],
        )
        self.assertIn(
            {"type": "toml", "path": "uv.lock", "jsonpath": "$.package[?(@.name.value=='cua-sandbox')].version"},
            package["extra-files"],
        )
        self.assertRegex(
            (SANDBOX_ROOT / "cua_sandbox/__init__.py").read_text(),
            r'(?m)^__version__ = "[^"]+"  # x-release-please-version$',
        )

    def test_legacy_bump_config_is_removed(self) -> None:
        self.assertFalse((SANDBOX_ROOT / ".bumpversion.cfg").exists())

    def test_legacy_workflows_cannot_bump_sandbox(self) -> None:
        workflow = (REPO_ROOT / ".github/workflows/release-bump-version.yml").read_text()
        self.assertNotIn("          - pypi/sandbox\n", workflow)
        self.assertNotIn('"pypi/sandbox")', workflow)
        self.assertNotIn("id: sandbox_version", workflow)
        self.assertIn("          - pypi/sandbox-apps\n", workflow)
        auto_release = (REPO_ROOT / ".github/workflows/release-on-merge.yml").read_text()
        self.assertNotIn('["libs/python/cua-sandbox/"]', auto_release)

    def test_release_branch_sync_recognizes_sandbox_and_preserves_manifest(self) -> None:
        workflow = (REPO_ROOT / ".github/workflows/release-please.yml").read_text()
        self.assertIn("          - sandbox\n", workflow)
        self.assertIn('.files | any(.path == "libs/python/cua-sandbox/VERSION")', workflow)
        self.assertIn('COMPONENT_ARGS+=(--component sandbox)', workflow)
        self.assertIn('elif [[ "$LUME" == "true" ]]', workflow)
        self.assertIn("--product sandbox", workflow)
        self.assertIn('"$BRANCH" != release-please--branches--*', workflow)
        self.assertIn('echo "Skipping non-release PR #$number; no branch code will run"\n              continue', workflow)

    def test_metadata_ci_covers_sandbox_and_offline_engine_preview(self) -> None:
        metadata = (REPO_ROOT / ".github/workflows/ci-release-metadata.yml").read_text()
        self.assertIn("libs/python/cua-sandbox/*)", metadata)
        self.assertIn("--tag-prefix sandbox-v", metadata)
        ci = (REPO_ROOT / ".github/workflows/ci-test-scripts.yml").read_text()
        self.assertIn('".github/workflows/cd-py-sandbox.yml"', ci)
        self.assertIn("release_please_sandbox_preview.cjs", ci)
        self.assertIn("--ignore-scripts", ci)


if __name__ == "__main__":
    unittest.main()

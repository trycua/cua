"""Regression tests for cua-cloud release automation wiring."""

from pathlib import Path
import unittest


REPO_ROOT = Path(__file__).resolve().parents[3]


class TestCuaCloudReleaseWiring(unittest.TestCase):
    """Verify cua-cloud is wired into the release automation."""

    def read(self, relative_path: str) -> str:
        return (REPO_ROOT / relative_path).read_text()

    def test_release_on_merge_tracks_cua_cloud(self) -> None:
        workflow = self.read(".github/workflows/release-on-merge.yml")

        self.assertIn('["libs/python/cua-cloud/"]="pypi/cloud"', workflow)

    def test_release_bump_version_supports_cua_cloud(self) -> None:
        workflow = self.read(".github/workflows/release-bump-version.yml")

        self.assertIn("- pypi/cloud", workflow)
        self.assertIn('"pypi/cloud")', workflow)
        self.assertIn('echo "directory=libs/python/cua-cloud" >> $GITHUB_OUTPUT', workflow)

    def test_unreleased_digest_tracks_cua_cloud(self) -> None:
        workflow = self.read(".github/workflows/release-unreleased-digest.yml")

        self.assertIn('SERVICE_TAG_DIR["pypi/cloud"]="cloud-v|libs/python/cua-cloud/"', workflow)

    def test_package_release_files_exist(self) -> None:
        self.assertTrue((REPO_ROOT / ".github/workflows/cd-py-cloud.yml").exists())
        self.assertTrue((REPO_ROOT / "libs/python/cua-cloud/.bumpversion.cfg").exists())


if __name__ == "__main__":
    unittest.main()

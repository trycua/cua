"""Regression checks for cursor-theme release and installer compatibility."""

from pathlib import Path
import unittest


REPO_ROOT = Path(__file__).resolve().parents[3]


class TestCursorThemeInstallerCompatibility(unittest.TestCase):
    def read(self, relative_path: str) -> str:
        return (REPO_ROOT / relative_path).read_text()

    def test_release_workflow_validates_archives_before_publish(self) -> None:
        workflow = self.read(".github/workflows/cd-rust-cua-driver.yml")
        validation = workflow.index("Validate native release archive payloads")

        self.assertLess(validation, workflow.index("Generate SHA256 checksums"))
        self.assertLess(
            validation, workflow.index("Publish the verified Release Please draft")
        )

    def test_unix_installer_only_relaxes_historical_releases(self) -> None:
        installer = self.read("libs/cua-driver/scripts/_install-rust.sh")

        self.assertIn('CURSOR_THEME_REQUIRED_FROM="0.12.7"', installer)
        self.assertIn(
            'version_is_at_least \\\n    "$VERSION" "$CURSOR_THEME_REQUIRED_FROM"',
            installer,
        )
        self.assertIn('if [[ "$THEME_AVAILABLE" == "1" ]]', installer)
        self.assertIn("predates cua-cursor-theme", installer)

    def test_windows_installer_only_relaxes_historical_releases(self) -> None:
        installer = self.read("libs/cua-driver/scripts/install.ps1")

        self.assertIn('$CursorThemeRequiredFrom = [version]"0.12.7"', installer)
        self.assertIn(
            "if ([version]$version -ge $CursorThemeRequiredFrom)", installer
        )
        self.assertIn("predates $ThemeBinaryName", installer)

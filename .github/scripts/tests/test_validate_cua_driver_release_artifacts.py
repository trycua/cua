"""Tests for the Cua Driver release archive payload guard."""

from pathlib import Path
import importlib.util
import tarfile
import tempfile
import unittest
import zipfile


SCRIPT = Path(__file__).resolve().parents[1] / "validate_cua_driver_release_artifacts.py"
SPEC = importlib.util.spec_from_file_location("artifact_validator", SCRIPT)
assert SPEC and SPEC.loader
VALIDATOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VALIDATOR)


class TestCuaDriverReleaseArtifacts(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.asset_dir = Path(self.temp_dir.name)
        self.version = "9.8.7"

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    def write_archives(self, omitted: tuple[str, str] | None = None) -> None:
        for filename, members in VALIDATOR.expected_archives(self.version).items():
            excluded = (
                {omitted[1]} if omitted and omitted[0] == filename else set()
            )
            selected = members - excluded
            path = self.asset_dir / filename
            if filename.endswith(".tar.gz"):
                with tarfile.open(path, "w:gz") as archive:
                    for member in sorted(selected):
                        info = tarfile.TarInfo(member)
                        info.size = 0
                        archive.addfile(info)
            else:
                with zipfile.ZipFile(path, "w") as archive:
                    for member in sorted(selected):
                        archive.writestr(member, "")

    def test_accepts_complete_linux_windows_and_darwin_archives(self) -> None:
        self.write_archives()

        VALIDATOR.validate_release_artifacts(self.asset_dir, self.version)

    def test_rejects_missing_top_level_cursor_theme_on_each_platform(self) -> None:
        cases = (
            (
                f"cua-driver-rs-{self.version}-linux-arm64-binary.tar.gz",
                "cua-cursor-theme",
            ),
            (
                f"cua-driver-rs-{self.version}-windows-x86_64-binary.zip",
                "cua-cursor-theme.exe",
            ),
            (
                f"cua-driver-rs-{self.version}-darwin-universal.tar.gz",
                f"cua-driver-rs-{self.version}-darwin-universal/cua-cursor-theme",
            ),
        )
        for filename, member in cases:
            with self.subTest(filename=filename):
                self.write_archives((filename, member))
                with self.assertRaisesRegex(
                    VALIDATOR.ArtifactValidationError, "cua-cursor-theme"
                ):
                    VALIDATOR.validate_release_artifacts(self.asset_dir, self.version)

    def test_rejects_missing_cursor_theme_inside_macos_app(self) -> None:
        filename = f"cua-driver-rs-{self.version}-darwin-arm64.tar.gz"
        member = (
            f"cua-driver-rs-{self.version}-darwin-arm64/"
            "CuaDriver.app/Contents/MacOS/cua-cursor-theme"
        )
        self.write_archives((filename, member))

        with self.assertRaisesRegex(
            VALIDATOR.ArtifactValidationError,
            "CuaDriver.app/Contents/MacOS/cua-cursor-theme",
        ):
            VALIDATOR.validate_release_artifacts(self.asset_dir, self.version)

    def test_rejects_any_missing_required_archive(self) -> None:
        self.write_archives()
        missing = self.asset_dir / (
            f"cua-driver-rs-{self.version}-windows-arm64.zip"
        )
        missing.unlink()

        with self.assertRaisesRegex(
            VALIDATOR.ArtifactValidationError, "archive is missing"
        ):
            VALIDATOR.validate_release_artifacts(self.asset_dir, self.version)

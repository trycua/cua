"""Unit tests for check_release_manifest.py (python3 -m unittest discover -s scripts/install/tests)."""

import hashlib
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent.parent
CHECK = HERE / "check_release_manifest.py"
BUILD = HERE / "release_manifest.py"
V = "0.8.0-beta.6"
BASE = f"https://github.com/trycua/cua/releases/download/cua-spaces-v{V}"


class CheckReleaseManifestTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self.tmp.name)
        for name in (
            f"cua-spaces-{V}-darwin-universal.dmg",
            f"cua-spaces-{V}-darwin-universal.dmg.sigstore.json",
            f"Cua-Spaces-{V}-universal.dmg",
            f"Cua-Spaces-{V}-universal.dmg.blockmap",
            f"Cua-Spaces-Setup-{V}-x64.exe",
            f"Cua-Spaces-{V}-x86_64.AppImage",
            f"cua-spaces_{V}_arm64.deb",
        ):
            (self.dir / name).write_bytes(name.encode())

    def tearDown(self):
        self.tmp.cleanup()

    def publish(self):
        lines = [
            f"{hashlib.sha256(p.read_bytes()).hexdigest()}  {p.name}"
            for p in sorted(self.dir.iterdir())
            if p.name not in ("checksums.txt", "release-artifacts.json")
        ]
        (self.dir / "checksums.txt").write_text("\n".join(lines) + "\n")
        out = subprocess.run(
            [sys.executable, str(BUILD), "--component", "app", "--version", V, "--dir", str(self.dir),
             "--base-url", BASE, "--out", str(self.dir / "release-artifacts.json")],
            capture_output=True, text=True,
        )
        self.assertEqual(out.returncode, 0, out.stderr)

    def check(self):
        return subprocess.run(
            [sys.executable, str(CHECK), "--dir", str(self.dir), "--component", "app", "--version", V, "--base-url", BASE],
            capture_output=True, text=True,
        )

    def test_a_complete_release_passes(self):
        self.publish()
        out = self.check()
        self.assertEqual(out.returncode, 0, out.stdout)

    def test_an_unlisted_windows_installer_fails(self):
        # CUA-1507: the manifests listed only the SwiftUI app's assets.
        self.publish()
        manifest = json.loads((self.dir / "release-artifacts.json").read_text())
        manifest["artifacts"] = [a for a in manifest["artifacts"] if not a["name"].endswith(".exe")]
        (self.dir / "release-artifacts.json").write_text(json.dumps(manifest))
        lines = (self.dir / "checksums.txt").read_text().splitlines()
        (self.dir / "checksums.txt").write_text("\n".join(line for line in lines if ".exe" not in line) + "\n")
        out = self.check()
        self.assertEqual(out.returncode, 1)
        self.assertIn(f"FAIL checksums.txt does not list Cua-Spaces-Setup-{V}-x64.exe", out.stdout)
        self.assertIn(f"FAIL release-artifacts.json does not list Cua-Spaces-Setup-{V}-x64.exe", out.stdout)

    def test_a_changed_file_fails_both_checksums(self):
        self.publish()
        (self.dir / f"Cua-Spaces-{V}-x86_64.AppImage").write_bytes(b"changed")
        out = self.check()
        self.assertEqual(out.returncode, 1)
        self.assertIn(f"FAIL checksums.txt: Cua-Spaces-{V}-x86_64.AppImage sha256", out.stdout)
        self.assertIn(f"FAIL release-artifacts.json: Cua-Spaces-{V}-x86_64.AppImage (linux-x64) sha256", out.stdout)

    def test_a_listed_file_missing_from_the_release_fails(self):
        self.publish()
        (self.dir / f"cua-spaces_{V}_arm64.deb").unlink()
        out = self.check()
        self.assertEqual(out.returncode, 1)
        self.assertIn(f"FAIL release-artifacts.json lists cua-spaces_{V}_arm64.deb, which is not on the release", out.stdout)


if __name__ == "__main__":
    unittest.main()

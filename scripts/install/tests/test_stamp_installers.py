"""Unit tests for stamp_installers.py (python3 -m unittest discover scripts/install/tests)."""

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent.parent
TOOL = HERE / "stamp_installers.py"


def run(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run([sys.executable, str(TOOL), *args], capture_output=True, text=True)


class StampInstallersTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.out = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def test_staging_repository_becomes_the_default(self):
        result = run("--repo", "trycua/cua-staging", "--out", str(self.out))
        self.assertEqual(result.returncode, 0, result.stderr)
        sh = (self.out / "install.sh").read_text()
        ps1 = (self.out / "install.ps1").read_text()
        self.assertIn('REPO="${CUA_INSTALL_REPO:-trycua/cua-staging}"', sh)
        self.assertIn("else { 'trycua/cua-staging' }", ps1)
        # The Sigstore identity and download base derive from REPO / $Repo.
        self.assertIn("https://github.com/%s/.github/workflows/cd-cua-sdk.yml", sh)
        self.assertIn("https://github.com/$Repo/.github/workflows/cd-cua-sdk.yml", ps1)
        self.assertEqual((self.out / "install.sh").stat().st_mode, (HERE / "install.sh").stat().st_mode)

    def test_canonical_repository_is_byte_identical(self):
        result = run("--repo", "trycua/cua", "--out", str(self.out))
        self.assertEqual(result.returncode, 0, result.stderr)
        for name in ("install.sh", "install.ps1"):
            self.assertEqual((self.out / name).read_bytes(), (HERE / name).read_bytes(), name)

    def test_rejects_a_bad_repository(self):
        for bad in ("", "trycua", "a/b/c", "trycua/cua; rm -rf /", "trycua/cua'"):
            self.assertNotEqual(run("--repo", bad, "--out", str(self.out)).returncode, 0, bad)

    def test_fails_when_the_default_line_is_gone(self):
        src = self.out / "src"
        src.mkdir()
        (src / "install.sh").write_text('REPO="${SOMETHING_ELSE:-trycua/cua}"\n')
        (src / "install.ps1").write_text((HERE / "install.ps1").read_text())
        result = run("--repo", "trycua/cua-staging", "--out", str(self.out / "o"), "--src", str(src))
        self.assertEqual(result.returncode, 1)
        self.assertIn("install.sh", result.stderr)


if __name__ == "__main__":
    unittest.main()

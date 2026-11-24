"""Unit tests for updater_feed.py (python3 -m unittest discover scripts/install/tests)."""

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

TOOL = Path(__file__).resolve().parent.parent / "updater_feed.py"
BASE = "https://github.com/trycua/cua/releases/download/cua-spaces-v1.2.3"


def run(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run([sys.executable, str(TOOL), *args], capture_output=True, text=True)


class UpdaterFeedTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self.tmp.name)
        self.out = self.dir / "latest.json"

    def tearDown(self):
        self.tmp.cleanup()

    def artifact(self, suffix: str, sig: str | None = "sig") -> None:
        name = f"cua-spaces-1.2.3-{suffix}"
        (self.dir / name).write_bytes(b"x")
        if sig is not None:
            (self.dir / f"{name}.sig").write_text(f"{sig}-{suffix}\n")

    def earlier(self, platforms: dict | None = None, version: str = "1.2.3") -> Path:
        path = self.dir / "earlier.json"
        path.write_text(json.dumps({
            "version": version,
            "notes": "notes",
            "pub_date": "2026-09-28T00:00:00Z",
            "platforms": platforms or {},
        }))
        return path

    def all_artifacts(self) -> None:
        for suffix in ("linux-x64.AppImage", "linux-arm64.AppImage", "windows-x64-setup.exe", "windows-x64.msi"):
            self.artifact(suffix)

    def test_lists_linux_and_windows_and_no_darwin(self):
        self.all_artifacts()
        result = run("--version", "1.2.3", "--dir", str(self.dir), "--base-url", BASE, "--out", str(self.out))
        self.assertEqual(result.returncode, 0, result.stderr)
        feed = json.loads(self.out.read_text())
        platforms = feed["platforms"]
        for key in ("linux-x86_64", "linux-aarch64", "windows-x86_64", "windows-x86_64-nsis", "windows-x86_64-msi"):
            self.assertIn(key, platforms)
        self.assertFalse([key for key in platforms if key.startswith("darwin")])
        self.assertEqual(platforms["windows-x86_64"]["url"], f"{BASE}/cua-spaces-1.2.3-windows-x64-setup.exe")
        self.assertEqual(platforms["linux-x86_64"]["signature"], "sig-linux-x64.AppImage")

    def test_merge_keeps_notes_and_date(self):
        self.all_artifacts()
        result = run("--version", "1.2.3", "--dir", str(self.dir), "--base-url", BASE,
                     "--merge", str(self.earlier()), "--out", str(self.out))
        self.assertEqual(result.returncode, 0, result.stderr)
        feed = json.loads(self.out.read_text())
        self.assertEqual(feed["pub_date"], "2026-09-28T00:00:00Z")
        self.assertEqual(feed["notes"], "notes")

    def test_darwin_entry_fails(self):
        # The macOS app is the SwiftUI app: the feed must never send a Mac a
        # Tauri build.
        self.all_artifacts()
        for key in ("darwin-universal", "darwin-aarch64", "darwin-x86_64"):
            merge = self.earlier({key: {"signature": "mac", "url": f"{BASE}/Cua.Spaces.app.tar.gz"}})
            result = run("--version", "1.2.3", "--dir", str(self.dir), "--base-url", BASE,
                         "--merge", str(merge), "--out", str(self.out))
            self.assertNotEqual(result.returncode, 0, key)
            self.assertIn("SwiftUI", result.stderr)
            self.assertFalse(self.out.exists())

    def test_missing_platform_fails(self):
        self.artifact("linux-x64.AppImage")
        result = run("--version", "1.2.3", "--dir", str(self.dir), "--base-url", BASE, "--out", str(self.out))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("missing platforms", result.stderr)
        self.assertFalse(self.out.exists())

    def test_unsigned_artifact_fails(self):
        self.all_artifacts()
        (self.dir / "cua-spaces-1.2.3-windows-x64.msi.sig").unlink()
        result = run("--version", "1.2.3", "--dir", str(self.dir), "--base-url", BASE, "--out", str(self.out))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("signature", result.stderr)

    def test_version_mismatch_fails(self):
        self.all_artifacts()
        result = run("--version", "1.2.3", "--dir", str(self.dir), "--base-url", BASE,
                     "--merge", str(self.earlier(version="9.9.9")), "--out", str(self.out))
        self.assertNotEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main()

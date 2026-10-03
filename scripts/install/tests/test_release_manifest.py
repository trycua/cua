"""Unit tests for release_manifest.py (python3 -m unittest discover scripts/install/tests)."""

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

TOOL = Path(__file__).resolve().parent.parent / "release_manifest.py"


def run(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run([sys.executable, str(TOOL), *args], capture_output=True, text=True)


class ReleaseManifestTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def touch(self, name: str, body: bytes = b"x") -> None:
        (self.dir / name).write_bytes(body)

    def test_cli_entries_one_per_line_with_checksums(self):
        self.touch("cua-cli-1.2.3-linux-x64.tar.gz", b"abc")
        self.touch("cua-cli-1.2.3-windows-arm64.zip")
        self.touch("cua-cli-1.2.3-linux-x64.tar.gz.minisig")
        self.touch("checksums.txt")
        out = run("--component", "cli", "--version", "1.2.3", "--dir", str(self.dir), "--base-url", "https://e/x")
        self.assertEqual(out.returncode, 0, out.stderr)
        data = json.loads(out.stdout)
        self.assertEqual(data["schema"], 1)
        self.assertEqual(data["versions"], {"cli": "1.2.3"})
        linux = next(a for a in data["artifacts"] if a["platform"] == "linux-x64")
        self.assertEqual(linux["sha256"], "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        self.assertEqual(linux["url"], "https://e/x/cua-cli-1.2.3-linux-x64.tar.gz")
        self.assertEqual(linux["minisig"], "https://e/x/cua-cli-1.2.3-linux-x64.tar.gz.minisig")
        self.assertEqual(linux["kind"], "tar.gz")
        rows = [line for line in out.stdout.splitlines() if '"component"' in line]
        self.assertEqual(len(rows), 2)
        for row in rows:
            json.loads(row.strip().rstrip(","))

    def test_universal_dmg_listed_for_both_macs_and_merge_keeps_other_component(self):
        self.touch("cua-cli-0.2.0-darwin-arm64.tar.gz")
        cli = self.dir / "cli.json"
        self.assertEqual(run("--component", "cli", "--version", "0.2.0", "--dir", str(self.dir), "--out", str(cli)).returncode, 0)
        self.touch("cua-spaces-0.9.0-darwin-universal.dmg")
        self.touch("cua-spaces-0.9.0-windows-x64-setup.exe")
        out = run("--component", "app", "--version", "0.9.0", "--dir", str(self.dir), "--merge", str(cli))
        self.assertEqual(out.returncode, 0, out.stderr)
        data = json.loads(out.stdout)
        self.assertEqual(data["versions"], {"cli": "0.2.0", "app": "0.9.0"})
        kinds = {(a["component"], a["platform"], a["kind"]) for a in data["artifacts"]}
        self.assertIn(("app", "darwin-arm64", "dmg"), kinds)
        self.assertIn(("app", "darwin-x64", "dmg"), kinds)
        self.assertIn(("app", "windows-x64", "nsis"), kinds)
        self.assertIn(("cli", "darwin-arm64", "tar.gz"), kinds)

    def test_version_mismatch_and_empty_dir_fail(self):
        self.touch("cua-cli-1.0.0-linux-x64.tar.gz")
        self.assertNotEqual(run("--component", "cli", "--version", "1.0.1", "--dir", str(self.dir)).returncode, 0)
        self.assertNotEqual(run("--component", "app", "--version", "1.0.0", "--dir", str(self.dir)).returncode, 0)

    def test_rejects_quotes_in_urls(self):
        self.touch("cua-cli-1.0.0-linux-x64.tar.gz")
        out = run("--component", "cli", "--version", "1.0.0", "--dir", str(self.dir), "--base-url", 'https://e/"x')
        self.assertNotEqual(out.returncode, 0)


if __name__ == "__main__":
    unittest.main()

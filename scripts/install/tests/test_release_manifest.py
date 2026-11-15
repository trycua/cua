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

    def test_electron_installers_for_every_platform_under_their_own_kinds(self):
        # CUA-1507: a prerelease's Electron installers were missing.
        v = "0.8.0-beta.6"
        names = {
            f"cua-spaces-{v}-darwin-universal.dmg": [("darwin-arm64", "dmg"), ("darwin-x64", "dmg")],
            f"Cua-Spaces-{v}-arm64.dmg": [("darwin-arm64", "electron-dmg")],
            f"Cua-Spaces-{v}-universal.dmg": [("darwin-arm64", "electron-dmg-universal"), ("darwin-x64", "electron-dmg-universal")],
            f"Cua-Spaces-{v}-arm64-mac.zip": [("darwin-arm64", "electron-zip")],
            f"Cua-Spaces-{v}-universal-mac.zip": [("darwin-arm64", "electron-zip-universal"), ("darwin-x64", "electron-zip-universal")],
            f"Cua-Spaces-{v}-x86_64.AppImage": [("linux-x64", "electron-appimage")],
            f"Cua-Spaces-{v}-arm64.AppImage": [("linux-arm64", "electron-appimage")],
            f"cua-spaces_{v}_amd64.deb": [("linux-x64", "electron-deb")],
            f"cua-spaces_{v}_arm64.deb": [("linux-arm64", "electron-deb")],
            f"Cua-Spaces-Setup-{v}.exe": [("windows-x64", "electron-nsis-universal"), ("windows-arm64", "electron-nsis-universal")],
            f"Cua-Spaces-Setup-{v}-x64.exe": [("windows-x64", "electron-nsis")],
            f"Cua-Spaces-Setup-{v}-arm64.exe": [("windows-arm64", "electron-nsis")],
        }
        for name in names:
            self.touch(name)
        self.touch(f"Cua-Spaces-{v}-arm64.dmg.blockmap")
        out = run("--component", "app", "--version", v, "--dir", str(self.dir))
        self.assertEqual(out.returncode, 0, out.stderr)
        got: dict = {}
        for a in json.loads(out.stdout)["artifacts"]:
            self.assertEqual(a["version"], v)
            got.setdefault(a["name"], []).append((a["platform"], a["kind"]))
        self.assertEqual({k: sorted(x) for k, x in got.items()}, {k: sorted(x) for k, x in names.items()})
        # The installers' kinds still name exactly one artifact per platform.
        kinds = [(p, k) for rows in got.values() for p, k in rows]
        self.assertEqual(len(kinds), len(set(kinds)))

    def test_installer_named_copies_of_the_electron_app_list_beside_the_electron_names(self):
        # A stable release: Windows and Linux Electron installers also under the
        # installer names, so install.sh / install.ps1 pick them (kinds nsis,
        # appimage, deb) while the electron-builder names stay (electron-*).
        v = "1.2.3"
        files = {
            f"Cua-Spaces-Setup-{v}-x64.exe": ("windows-x64", "electron-nsis"),
            f"cua-spaces-{v}-windows-x64-setup.exe": ("windows-x64", "nsis"),
            f"cua-spaces-{v}-windows-arm64-setup.exe": ("windows-arm64", "nsis"),
            f"Cua-Spaces-{v}-x86_64.AppImage": ("linux-x64", "electron-appimage"),
            f"cua-spaces-{v}-linux-x64.AppImage": ("linux-x64", "appimage"),
            f"cua-spaces_{v}_amd64.deb": ("linux-x64", "electron-deb"),
            f"cua-spaces-{v}-linux-x64.deb": ("linux-x64", "deb"),
            f"cua-spaces-{v}-linux-arm64.deb": ("linux-arm64", "deb"),
        }
        for name in files:
            self.touch(name, name.encode())
        out = self.dir / "m.json"
        proc = run("--component", "app", "--version", v, "--dir", str(self.dir), "--out", str(out))
        self.assertEqual(proc.returncode, 0, proc.stderr)
        got = {(a["name"], a["platform"], a["kind"]) for a in json.loads(out.read_text())["artifacts"]}
        self.assertEqual(got, {(n, *pk) for n, pk in files.items()})

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

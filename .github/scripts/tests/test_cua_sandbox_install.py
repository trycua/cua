"""Exercise the action's public setup phase using local release fixtures."""

import hashlib
import io
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[3]
ACTION = ROOT / ".github/actions/cua-sandbox/cua-sandbox.sh"
VERSION = "0.31.0"


@unittest.skipIf(os.name == "nt", "the sandbox action runs on Unix hosts")
class SandboxReleaseInstall(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(prefix="cua-release-test-")
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        self.release = self.root / "release"
        self.bin = self.root / "bin"
        self.release.mkdir()
        self.bin.mkdir()
        self.requests = self.root / "requests"
        self.shim("uname", """import os, sys
print(os.environ['TEST_OS' if sys.argv[1] == '-s' else 'TEST_ARCH'])
""")
        self.shim("curl", """import os, pathlib, shutil, sys
args = sys.argv[1:]
name = args[-1].rsplit('/', 1)[-1]
with open(os.environ['TEST_REQUESTS'], 'a') as log:
    log.write(name + '\\n')
source = pathlib.Path(os.environ['TEST_RELEASE']) / name
if not source.is_file():
    sys.exit(22)
shutil.copyfile(source, args[args.index('-o') + 1])
""")

    def shim(self, name, source):
        path = self.bin / name
        path.write_text(f"#!{sys.executable}\n" + source)
        path.chmod(0o755)

    def archive(self, platform, marker="fixture", checksum=True):
        name = f"cua-cli-{VERSION}-{platform}.tar.gz"
        path = self.release / name
        program = f"#!/bin/sh\nprintf 'cua {marker}\\n'\n".encode()
        with tarfile.open(path, "w:gz") as archive:
            member = tarfile.TarInfo("cua")
            member.mode = 0o755
            member.size = len(program)
            archive.addfile(member, io.BytesIO(program))
        if checksum:
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            with (self.release / "checksums.txt").open("a") as manifest:
                manifest.write(f"{digest}  {name}\n")
        return name

    def setup_action(self, os_name="Linux", arch="x86_64"):
        env = os.environ.copy()
        env.update(
            PATH=str(self.bin) + os.pathsep + env["PATH"],
            TEST_OS=os_name, TEST_ARCH=arch,
            TEST_RELEASE=str(self.release), TEST_REQUESTS=str(self.requests),
            CUA_BIN="", CUA_SB_CUA_VERSION=VERSION, CUA_SB_ON="cloud",
            CUA_SB_STATE=str(self.root / "state"), CUA_SB_NAME="release-fixture",
            CUA_SB_RUNTIME="", CUA_SB_OVERLAY="",
            GITHUB_OUTPUT=str(self.root / "output"),
        )
        return subprocess.run(["bash", str(ACTION), "setup"], env=env,
                              text=True, capture_output=True, timeout=20)

    def requested(self):
        return self.requests.read_text().splitlines()

    def test_current_linux_assets(self):
        for arch, platform in [("x86_64", "linux-x64"),
                               ("aarch64", "linux-arm64")]:
            with self.subTest(arch=arch):
                name = self.archive(platform)
                result = self.setup_action(arch=arch)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn(name, self.requested())

    def test_linux_arm64_uname_alias(self):
        self.archive("linux-arm64")
        result = self.setup_action(arch="arm64")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_legacy_linux_assets(self):
        for arch, platform in [("x86_64", "linux-x64-gnu"),
                               ("aarch64", "linux-arm64-gnu")]:
            with self.subTest(arch=arch):
                self.archive(platform)
                result = self.setup_action(arch=arch)
                self.assertEqual(result.returncode, 0, result.stderr)

    def test_plain_asset_wins_when_both_are_published(self):
        plain = self.archive("linux-x64", marker="plain")
        legacy = self.archive("linux-x64-gnu", marker="legacy")
        result = self.setup_action()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("cua plain", result.stderr)
        self.assertIn(plain, self.requested())
        self.assertNotIn(legacy, self.requested())

    def test_darwin_assets_are_unchanged(self):
        for arch in ["arm64", "x86_64"]:
            with self.subTest(arch=arch):
                self.archive("darwin-arm64" if arch == "arm64" else "darwin-x64")
                result = self.setup_action(os_name="Darwin", arch=arch)
                self.assertEqual(result.returncode, 0, result.stderr)

    def test_checksum_mismatch_does_not_fall_back_or_extract(self):
        plain = self.archive("linux-x64")
        legacy = self.archive("linux-x64-gnu")
        (self.release / plain).write_bytes(b"corrupt archive")
        result = self.setup_action()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("checksum mismatch", result.stderr)
        self.assertNotIn(legacy, self.requested())
        self.assertFalse((self.root / f"state/cua-{VERSION}/cua").exists())

    def test_missing_manifest_entry_does_not_download_archive(self):
        self.archive("linux-x64", checksum=False)
        (self.release / "checksums.txt").write_text("")
        result = self.setup_action()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.requested(), ["checksums.txt"])

    def test_missing_checksums_does_not_download_archive(self):
        self.archive("linux-x64", checksum=False)
        result = self.setup_action()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.requested(), ["checksums.txt"])

    def test_missing_selected_archive_does_not_try_other_contents(self):
        plain = self.archive("linux-x64")
        legacy = self.archive("linux-x64-gnu")
        (self.release / plain).unlink()
        result = self.setup_action()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(plain, self.requested())
        self.assertNotIn(legacy, self.requested())


if __name__ == "__main__":
    unittest.main()

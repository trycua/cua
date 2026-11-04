"""Partial (per-platform) releases: install.sh and install.ps1 take the newest
build that exists for their own platform (CUA-2068; python3 -m unittest
discover -s scripts/install/tests). install.ps1 runs only on Windows: its half
is in install.Tests.ps1 ('mixed versions'), built the same way.

The manifest is built the way cd-cua-spaces.yml builds it: release 1.2.3 ships
every platform, 1.2.4 only macOS and 1.2.5 only Linux, each merged into the
previous manifest with --carry-forward-platforms. The result holds three
versions at once. A loopback server plays GitHub's release downloads, one
directory per release tag.
"""

import http.server
import json
import os
import shutil
import subprocess
import tempfile
import threading
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
SH = HERE.parent / "install.sh"
PS1 = HERE.parent / "install.ps1"
TOOL = HERE.parent / "release_manifest.py"
CHECK = HERE.parent / "check_release_manifest.py"

FILES = {
    "1.2.3": [
        "cua-spaces-{v}-darwin-universal.dmg",
        "cua-spaces-{v}-linux-x64.AppImage",
        "cua-spaces-{v}-linux-arm64.AppImage",
        "cua-spaces-{v}-linux-x64.deb",
        "cua-spaces-{v}-linux-arm64.deb",
        "cua-spaces-{v}-windows-x64-setup.exe",
        "cua-spaces-{v}-windows-arm64-setup.exe",
    ],
    "1.2.4": ["cua-spaces-{v}-darwin-universal.dmg"],
    "1.2.5": [
        "cua-spaces-{v}-linux-x64.AppImage",
        "cua-spaces-{v}-linux-arm64.AppImage",
        "cua-spaces-{v}-linux-x64.deb",
        "cua-spaces-{v}-linux-arm64.deb",
    ],
}


def tool(*args):
    return subprocess.run(["python3", str(TOOL), *args], check=True, capture_output=True, text=True)


class MixedVersions(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        root = Path(cls.tmp.name)
        cls.release = root / "release"

        class Quiet(http.server.SimpleHTTPRequestHandler):
            def log_message(self, *args):
                pass

        handler = lambda *a, **k: Quiet(*a, directory=str(cls.release), **k)  # noqa: E731
        cls.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
        threading.Thread(target=cls.server.serve_forever, daemon=True).start()
        cls.base = f"http://127.0.0.1:{cls.server.server_address[1]}"
        previous = None
        cls.manifests = {}
        for version, names in FILES.items():
            tag = cls.release / f"cua-spaces-v{version}"
            tag.mkdir(parents=True)
            for name in names:
                body = f"#!/bin/sh\necho cua-spaces {version}\n" if name.endswith(".AppImage") else f"{name}\n"
                (tag / name.format(v=version)).write_text(body)
            out = root / f"manifest-{version}.json"
            args = ["--component", "app", "--version", version, "--dir", str(tag),
                    "--base-url", f"{cls.base}/cua-spaces-v{version}", "--out", str(out)]
            if previous:
                args += ["--merge", str(previous), "--carry-forward-platforms"]
            tool(*args)
            cls.manifests[version] = out
            previous = out
        # What cua-install-latest holds after all three releases.
        latest = cls.release / "cua-install-latest"
        latest.mkdir()
        shutil.copy(previous, latest / "release-artifacts.json")
        cls.manifest_url = f"{cls.base}/cua-install-latest/release-artifacts.json"
        cls.manifest = json.loads(previous.read_text())

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.tmp.cleanup()

    def versions(self, manifest=None):
        found = {}
        for a in (manifest or self.manifest)["artifacts"]:
            found.setdefault(a["platform"], set()).add(a["version"])
        return found

    def test_manifest_holds_the_newest_build_per_platform(self):
        self.assertEqual(
            self.versions(),
            {
                "darwin-arm64": {"1.2.4"}, "darwin-x64": {"1.2.4"},
                "linux-arm64": {"1.2.5"}, "linux-x64": {"1.2.5"},
                "windows-arm64": {"1.2.3"}, "windows-x64": {"1.2.3"},
            },
        )
        self.assertEqual(self.manifest["versions"]["app"], "1.2.5")

    def test_carry_forward_is_opt_in(self):
        # Without the flag, the component's earlier artifacts are replaced, as before.
        out = Path(self.tmp.name) / "replaced.json"
        tool("--component", "app", "--version", "1.2.4", "--dir", str(self.release / "cua-spaces-v1.2.4"),
             "--merge", str(self.manifests["1.2.3"]), "--out", str(out))
        self.assertEqual(set(self.versions(json.loads(out.read_text()))), {"darwin-arm64", "darwin-x64"})

    def test_a_platform_with_nothing_to_carry_forward_is_an_error(self):
        p = subprocess.run(
            ["python3", str(TOOL), "--component", "app", "--version", "1.2.4",
             "--dir", str(self.release / "cua-spaces-v1.2.4"), "--carry-forward-platforms"],
            capture_output=True, text=True,
        )
        self.assertNotEqual(p.returncode, 0)
        self.assertIn("no previous app artifact", p.stderr)

    def test_checker_accepts_a_partial_release_only_when_allowed(self):
        tag = self.release / "cua-spaces-v1.2.4"
        work = Path(self.tmp.name) / "check"
        shutil.copytree(tag, work)
        shutil.copy(self.manifests["1.2.4"], work / "release-artifacts.json")
        import hashlib
        (work / "checksums.txt").write_text("".join(
            f"{hashlib.sha256(p.read_bytes()).hexdigest()}  {p.name}\n"
            for p in sorted(work.iterdir()) if p.name not in ("release-artifacts.json", "checksums.txt")))
        base = f"{self.base}/cua-spaces-v1.2.4"
        args = ["python3", str(CHECK), "--dir", str(work), "--component", "app", "--version", "1.2.4", "--base-url", base]
        self.assertNotEqual(subprocess.run(args, capture_output=True).returncode, 0)
        self.assertEqual(subprocess.run([*args, "--allow-carried-forward"], capture_output=True, text=True).returncode, 0)

    def sh(self, os_name, arch, prefix, *extra):
        env = {
            "PATH": os.environ["PATH"], "HOME": str(prefix), "CUA_HOME": str(prefix / ".cua"),
            "CUA_INSTALL_OS": os_name, "CUA_INSTALL_ARCH": arch,
            "CUA_INSTALL_MANIFEST_URL": self.manifest_url, "CUA_TELEMETRY_FORBID_NETWORK": "1",
        }
        return subprocess.run(
            ["sh", str(SH), "--yes", "--no-onboarding", "--app-only", "--prefix", str(prefix / "p"), *extra],
            env=env, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=120,
        )

    def test_install_sh_installs_the_newest_linux_build(self):
        for arch, label in (("x86_64", "linux-x64"), ("aarch64", "linux-arm64")):
            prefix = Path(tempfile.mkdtemp(dir=self.tmp.name))
            p = self.sh("linux", arch, prefix)
            self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
            ran = subprocess.run([str(prefix / "p/bin/cua-spaces")], capture_output=True, text=True)
            self.assertEqual(ran.stdout.strip(), "cua-spaces 1.2.5", label)

    def test_install_sh_plans_the_newest_macos_build(self):
        prefix = Path(tempfile.mkdtemp(dir=self.tmp.name))
        p = self.sh("Darwin", "arm64", prefix, "--dry-run")
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertIn("cua-spaces-v1.2.4/cua-spaces-1.2.4-darwin-universal.dmg", p.stdout)
        self.assertNotIn("1.2.3", p.stdout)


if __name__ == "__main__":
    unittest.main()

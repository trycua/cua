"""Platform planner and release notes for per-platform Spaces patches (CUA-2068)."""

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import spaces_platforms as sp  # noqa: E402

D = "apps/cua-spaces-desktop/"
BUMPS = [
    ".release-please-manifest.json",
    "apps/cua-spaces-macos/VERSION",
    "apps/cua-spaces-macos/CHANGELOG.md",
    "apps/cua-spaces-macos/Support/Info.plist",
    D + "package.json",
    "apps/cua-spaces/src-tauri/Cargo.lock",
]


def decide(paths, bumps_are_version_only=True):
    return sp.decide(paths, lambda p: bumps_are_version_only)[0]


class Decide(unittest.TestCase):
    def test_macos_only(self):
        self.assertEqual(decide(["apps/cua-spaces-macos/Sources/App.swift"]), ["macos"])

    def test_macos_with_release_bump_files_is_still_macos_only(self):
        self.assertEqual(decide(["apps/cua-spaces-macos/Sources/App.swift", *BUMPS]), ["macos"])

    def test_windows_and_linux_packaging(self):
        self.assertEqual(decide([D + "packaging/installer.nsh"]), ["windows"])
        self.assertEqual(decide([D + "packaging/linux/prerm.sh"]), ["linux"])
        self.assertEqual(decide([D + "packaging/linux/prerm.sh", D + "packaging/installer.nsh"]), ["windows", "linux"])
        self.assertEqual(decide([D + "packaging/installer.nsh", "apps/cua-spaces-macos/VERSION"]), ["windows"])

    def test_shared_paths_mean_all(self):
        for path in (
            "libs/cua/crates/cua-sdk/src/lib.rs",
            D + "src/main.ts",
            D + "src/model/spaces.ts",
            "apps/cua-spaces-web/src/App.tsx",
            "libs/cua-spacesd/VERSION",
            D + "electron-builder.config.cjs",
            D + "pnpm-lock.yaml",
            ".github/workflows/cd-cua-spaces.yml",
        ):
            self.assertEqual(decide([path]), list(sp.OSES), path)

    def test_one_shared_path_among_platform_paths_means_all(self):
        self.assertEqual(decide(["apps/cua-spaces-macos/Sources/A.swift", "libs/cua/Cargo.toml"]), list(sp.OSES))

    def test_unknown_path_means_all(self):
        self.assertEqual(decide(["scripts/brand-new-dir/thing.sh"]), list(sp.OSES))

    def test_nothing_or_neutral_only_means_all(self):
        self.assertEqual(sp.decide([])[0], list(sp.OSES))
        self.assertEqual(decide(["docs/a.mdx", "README.md", *BUMPS]), list(sp.OSES))

    def test_a_real_edit_to_a_bump_file_is_not_neutral(self):
        # Info.plist changed beyond the version: macOS owns it. package.json
        # beyond the version: shared.
        self.assertEqual(decide(["apps/cua-spaces-macos/Support/Info.plist"], False), ["macos"])
        self.assertEqual(decide([D + "package.json", "apps/cua-spaces-macos/Sources/A.swift"], False), list(sp.OSES))

    def test_desktop_tests_and_docs_are_neutral(self):
        self.assertEqual(decide([D + "test/teams.test.ts", D + "docs/x.md", D + "packaging/installer.nsh"]), ["windows"])

    def test_electron_mac_signing_is_macos(self):
        self.assertEqual(decide([D + "packaging/sign-mac.cjs", D + "packaging/entitlements.mac.plist"]), ["macos"])


class VersionOnly(unittest.TestCase):
    def test_json_bump(self):
        self.assertTrue(sp.version_only_diff('-  "version": "0.13.0",\n+  "version": "0.13.1",\n'))

    def test_plist_and_bare_version(self):
        self.assertTrue(sp.version_only_diff("-\t<string>0.13.0</string>\n+\t<string>0.13.1</string>\n"))
        self.assertTrue(sp.version_only_diff("-0.13.0\n+0.13.1\n"))

    def test_manifest_and_cargo(self):
        self.assertTrue(sp.version_only_diff('-  "apps/cua-spaces-macos": "0.13.0"\n+  "apps/cua-spaces-macos": "0.13.1"\n'))
        self.assertTrue(sp.version_only_diff('-version = "0.13.0"\n+version = "0.13.1"\n'))

    def test_other_edits_are_not_version_only(self):
        self.assertFalse(sp.version_only_diff('-  "version": "0.13.0",\n+  "version": "0.13.1",\n+  "scripts": {},\n'))
        self.assertFalse(sp.version_only_diff(""))


class Manual(unittest.TestCase):
    def args(self, **kw):
        base = dict(previous="", head="HEAD", manual="", reason="", force_all="")
        base.update(kw)
        return type("A", (), base)

    def test_manual_needs_reason(self):
        with self.assertRaises(SystemExit):
            sp.plan(self.args(manual="macos"))
        out = sp.plan(self.args(manual="macos", reason="notarization hotfix"))
        self.assertEqual((out["macos"], out["windows"], out["linux"], out["partial"]), ("true", "false", "false", "true"))
        self.assertIn("notarization hotfix", out["reason"])

    def test_manual_all_needs_no_reason(self):
        self.assertEqual(sp.plan(self.args(manual="all"))["partial"], "false")

    def test_bad_name(self):
        with self.assertRaises(SystemExit):
            sp.plan(self.args(manual="macos,beos", reason="x"))

    def test_defaults_to_all(self):
        self.assertEqual(sp.plan(self.args())["platforms"], "macos,windows,linux")
        self.assertEqual(sp.plan(self.args(force_all="prerelease"))["partial"], "false")
        self.assertEqual(sp.plan(self.args(previous="no-such-tag-xyz"))["partial"], "false")


class GitPlan(unittest.TestCase):
    """The planner end to end against a throwaway repository."""

    def run_git(self, *args):
        subprocess.run(["git", "-C", self.repo, *args], check=True, capture_output=True)

    def write(self, path, text):
        file = Path(self.repo) / path
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(text)

    def test_mac_fix_after_a_release(self):
        with tempfile.TemporaryDirectory() as repo:
            self.repo = repo
            self.run_git("init", "-q", "-b", "main")
            self.run_git("config", "user.email", "t@example.com")
            self.run_git("config", "user.name", "t")
            self.write("apps/cua-spaces-macos/VERSION", "0.13.0\n")
            self.write(D + "package.json", '{\n  "name": "x",\n  "version": "0.13.0"\n}\n')
            self.write(D + "src/main.ts", "a\n")
            self.run_git("add", "-A")
            self.run_git("commit", "-qm", "release 0.13.0")
            self.run_git("tag", "cua-spaces-v0.13.0")
            self.write("apps/cua-spaces-macos/Sources/A.swift", "fix\n")
            self.write("apps/cua-spaces-macos/VERSION", "0.13.1\n")
            self.write(D + "package.json", '{\n  "name": "x",\n  "version": "0.13.1"\n}\n')
            self.run_git("add", "-A")
            self.run_git("commit", "-qm", "mac fix and bump")
            cwd = Path.cwd()
            try:
                import os
                os.chdir(repo)
                out = sp.plan(type("A", (), dict(previous="cua-spaces-v0.13.0", head="HEAD", manual="", reason="", force_all="")))
                self.assertEqual(out["platforms"], "macos")
                self.write(D + "src/main.ts", "b\n")
                self.run_git("add", "-A")
                self.run_git("commit", "-qm", "renderer change")
                out = sp.plan(type("A", (), dict(previous="cua-spaces-v0.13.0", head="HEAD", manual="", reason="", force_all="")))
                self.assertEqual(out["platforms"], "macos,windows,linux")
            finally:
                os.chdir(cwd)


class Notes(unittest.TestCase):
    MANIFEST = {"artifacts": [
        {"component": "app", "platform": "darwin-arm64", "version": "0.13.1"},
        {"component": "app", "platform": "windows-x64", "version": "0.13.0"},
        {"component": "app", "platform": "linux-x64", "version": "0.12.2"},
    ]}

    def test_partial(self):
        text = sp.notes(["macos"], self.MANIFEST)
        self.assertIn("**Platforms in this release:** macOS only.", text)
        self.assertIn("Windows stays on 0.13.0; Linux stays on 0.12.2", text)
        self.assertIn("Windows and Linux users are not offered this update", text)

    def test_full_release_has_no_block(self):
        self.assertEqual(sp.notes(list(sp.OSES), self.MANIFEST), "")


if __name__ == "__main__":
    unittest.main()

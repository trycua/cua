"""Cua Spaces releases are driven by the shipped macOS app, not the Tauri or
Electron app; both share its version."""

import importlib.util
import json
from pathlib import Path
import unittest


REPO_ROOT = Path(__file__).resolve().parents[3]
MACOS = "apps/cua-spaces-macos"
TAURI = "apps/cua-spaces"
ELECTRON = "apps/cua-spaces-desktop"


def load_script(name: str):
    spec = importlib.util.spec_from_file_location(
        name, REPO_ROOT / ".github/scripts" / f"{name}.py"
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class TestCuaSpacesReleaseWiring(unittest.TestCase):
    def setUp(self) -> None:
        self.config = json.loads((REPO_ROOT / "release-please-config.json").read_text())
        self.manifest = json.loads(
            (REPO_ROOT / ".release-please-manifest.json").read_text()
        )

    def test_component_lives_on_the_macos_app(self) -> None:
        packages = self.config["packages"]
        spaces = [path for path, package in packages.items() if package["component"] == "cua-spaces"]
        self.assertEqual(spaces, [MACOS])
        # The Tauri app must not be a package: its commits never open a release PR.
        self.assertNotIn(TAURI, packages)
        self.assertNotIn(TAURI, self.manifest)
        # Nor the Electron app: its prereleases ride the cua-spaces version.
        self.assertNotIn(ELECTRON, packages)
        self.assertNotIn(ELECTRON, self.manifest)
        self.assertNotIn(ELECTRON, packages[MACOS].get("additional-paths", []))
        self.assertEqual(
            self.manifest[MACOS], (REPO_ROOT / MACOS / "VERSION").read_text().strip()
        )
        self.assertFalse((REPO_ROOT / TAURI / "VERSION").exists())
        self.assertTrue((REPO_ROOT / MACOS / "CHANGELOG.md").is_file())

    def test_tauri_and_electron_apps_and_info_plist_share_the_version(self) -> None:
        package = self.config["packages"][MACOS]
        self.assertEqual(package["version-file"], "VERSION")
        self.assertEqual(package["changelog-path"], "CHANGELOG.md")
        paths = {extra["path"] for extra in package["extra-files"]}
        self.assertEqual(
            paths,
            {
                "Support/Info.plist",
                f"/{TAURI}/package.json",
                f"/{TAURI}/src-tauri/tauri.conf.json",
                f"/{TAURI}/src-tauri/Cargo.toml",
                f"/{TAURI}/src-tauri/Cargo.lock",
                f"/{ELECTRON}/package.json",
            },
        )
        for extra in package["extra-files"]:
            path = extra["path"]
            on_disk = REPO_ROOT / (path[1:] if path.startswith("/") else f"{MACOS}/{path}")
            self.assertTrue(on_disk.is_file(), path)
        electron = json.loads((REPO_ROOT / ELECTRON / "package.json").read_text())
        self.assertEqual(electron["version"], self.manifest[MACOS])

    def test_release_scripts_resolve_every_component_to_its_package(self) -> None:
        expected = {
            package["component"]: path for path, package in self.config["packages"].items()
        }
        for name in ("resolve_release_please_request", "merge_release_please_manifests"):
            self.assertEqual(load_script(name).COMPONENT_PATHS, expected, name)
        self.assertEqual(set(self.manifest), set(self.config["packages"]))

    def test_release_workflow_reads_the_macos_changelog(self) -> None:
        workflow = (REPO_ROOT / ".github/workflows/cd-cua-spaces.yml").read_text()
        self.assertIn(f'changelog-notes.py "$VERSION" {MACOS}/CHANGELOG.md', workflow)
        self.assertNotIn(f"{TAURI}/CHANGELOG.md", workflow)
        self.assertNotIn(f"{TAURI}/VERSION", workflow)

    def test_ci_runs_the_offline_engine_preview(self) -> None:
        ci = (REPO_ROOT / ".github/workflows/ci-test-scripts.yml").read_text()
        self.assertIn("release_please_spaces_preview.cjs", ci)
        self.assertIn(f'"{MACOS}/VERSION"', ci)

    def test_nothing_is_published_before_verify_release_passes(self) -> None:
        workflow = (REPO_ROOT / ".github/workflows/cd-cua-spaces.yml").read_text()
        verify = workflow.split("\n  verify-release:\n", 1)[1].split("\n  installer-manifest:\n", 1)[0]
        # It runs on the signed and notarized build of every platform.
        self.assertIn(
            "needs: [build-macos, electron-plan, electron-mac, electron-windows, electron-publish]",
            verify,
        )
        self.assertIn("runs-on: macos-26", verify)
        self.assertIn(f"{MACOS}/scripts/verify-release.sh --dir release", verify)
        self.assertIn('--repo "$GITHUB_REPOSITORY"', verify)
        self.assertIn("brew install osslsigncode", verify)
        # installer-manifest publishes the release and the feeds only after it.
        publish = workflow.split("\n  installer-manifest:\n", 1)[1]
        self.assertIn("verify-release]", publish.split("\n", 3)[1])
        self.assertIn("needs.verify-release.result == 'success'", publish)
        # checksums.txt and release-artifacts.json cover every asset, the
        # Electron Windows and Linux installers included (CUA-1507).
        self.assertIn('gh release download "$TAG" --dir assets', publish)
        self.assertIn("scripts/install/check_release_manifest.py --dir assets --component app", publish)
        # verify-release reads the macOS build on tag runs too.
        self.assertNotIn("Keep the macOS build (manual runs)", workflow)
        script = (REPO_ROOT / MACOS / "scripts/verify-release.sh").read_text()
        for check in (
            "spctl -a -vv -t install",
            "xcrun stapler validate",
            "codesign --verify --deep --strict",
            "osslsigncode verify",
            "verify-appcast.sh",
        ):
            self.assertIn(check, script)
        self.assertTrue((REPO_ROOT / MACOS / "scripts/certs/microsoft-identity-verification-root-2020.pem").is_file())

    def test_cla_never_locks_a_release_please_pull_request(self) -> None:
        # Release Please comments on the merged release PR when it tags the
        # release; a locked one fails every run ("issue is locked").
        cla = (REPO_ROOT / ".github/workflows/cla.yml").read_text()
        self.assertIn("lock-pullrequest-aftermerge: false", cla)


if __name__ == "__main__":
    unittest.main()

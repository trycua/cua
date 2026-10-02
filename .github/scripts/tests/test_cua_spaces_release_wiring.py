"""Cua Spaces releases are driven by the shipped macOS app, not the Tauri app."""

import importlib.util
import json
from pathlib import Path
import unittest


REPO_ROOT = Path(__file__).resolve().parents[3]
MACOS = "apps/cua-spaces-macos"
TAURI = "apps/cua-spaces"


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
        self.assertEqual(
            self.manifest[MACOS], (REPO_ROOT / MACOS / "VERSION").read_text().strip()
        )
        self.assertFalse((REPO_ROOT / TAURI / "VERSION").exists())
        self.assertTrue((REPO_ROOT / MACOS / "CHANGELOG.md").is_file())

    def test_tauri_app_and_info_plist_share_the_version(self) -> None:
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
            },
        )
        for extra in package["extra-files"]:
            path = extra["path"]
            on_disk = REPO_ROOT / (path[1:] if path.startswith("/") else f"{MACOS}/{path}")
            self.assertTrue(on_disk.is_file(), path)

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


if __name__ == "__main__":
    unittest.main()

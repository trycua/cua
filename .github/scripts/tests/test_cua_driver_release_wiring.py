"""Regression tests for cua-driver-rs release and PyPI wiring."""

from pathlib import Path
import unittest


REPO_ROOT = Path(__file__).resolve().parents[3]


class TestCuaDriverReleaseWiring(unittest.TestCase):
    """Verify cua-driver-rs releases feed the Python cua-driver publisher."""

    def read(self, relative_path: str) -> str:
        return (REPO_ROOT / relative_path).read_text()

    def test_python_publish_follows_rust_workflow_run(self) -> None:
        workflow = self.read(".github/workflows/cd-py-cua-driver.yml")

        self.assertIn('workflows: ["CD: Cua Driver (cross-platform)"]', workflow)
        self.assertNotIn("branches:\n      - main", workflow)
        self.assertIn("github.event.workflow_run.conclusion != 'cancelled'", workflow)
        self.assertIn('gh release view "$TAG" --repo "$GITHUB_REPOSITORY"', workflow)

    def test_python_publish_defaults_to_current_rust_version(self) -> None:
        workflow = self.read(".github/workflows/cd-py-cua-driver.yml")

        self.assertIn("required: false", workflow)
        self.assertIn('default: ""', workflow)
        self.assertIn("libs/cua-driver/rust/Cargo.toml", workflow)

    def test_python_publish_builds_linux_arm64_wheel(self) -> None:
        workflow = self.read(".github/workflows/cd-py-cua-driver.yml")

        self.assertIn("os: ubuntu-24.04-arm", workflow)
        self.assertIn("arch: arm64", workflow)

    def test_release_please_owns_driver_and_lume(self) -> None:
        config = self.read("release-please-config.json")
        workflow = self.read(".github/workflows/release-please.yml")

        self.assertIn('"libs/cua-driver"', config)
        self.assertIn('"libs/lume"', config)
        self.assertIn('"component": "cua-driver-rs"', config)
        self.assertIn('"component": "lume"', config)
        self.assertIn("5c625bfb5d1ff62eadeeb3772007f7f66fdcf071", workflow)
        self.assertIn('-p cua-driver --precise "$DRIVER_VERSION"', workflow)

    def test_legacy_release_routes_exclude_driver_and_lume(self) -> None:
        workflow = self.read(".github/workflows/release-bump-version.yml")
        self.assertNotIn("          - cua-driver-rs\n", workflow)
        self.assertNotIn("          - lume\n", workflow)
        self.assertNotIn("gh api -X DELETE", workflow)
        self.assertIn("release tags are immutable", workflow)

        for path in (
            ".github/workflows/release-on-merge.yml",
            ".github/workflows/ci-release-reminder.yml",
            ".github/workflows/release-unreleased-digest.yml",
        ):
            legacy = self.read(path)
            self.assertNotIn('="cua-driver-rs"', legacy, path)
            self.assertNotIn('SERVICE_TAG_DIR["lume"]', legacy, path)

    def test_distro_compat_downloads_release_asset_once_per_run(self) -> None:
        workflow = self.read(".github/workflows/ci-distro-compat-cua-driver.yml")

        self.assertEqual(workflow.count('curl -fsSL "$BINARY_URL"'), 1)
        self.assertIn("actions/upload-artifact@v4", workflow)
        self.assertIn("actions/download-artifact@v4", workflow)
        self.assertIn("cua-driver-release-${{ steps.pick.outputs.version }}", workflow)
        self.assertIn('CUA_DRIVER_RS_TELEMETRY_ENABLED: "false"', workflow)
        self.assertIn('CUA_TELEMETRY_ENABLED: "false"', workflow)

    def test_release_please_keeps_driver_version_sources_synced(self) -> None:
        config = self.read("release-please-config.json")

        self.assertIn('"path": "rust/Cargo.toml"', config)
        self.assertIn('"path": "python/pyproject.toml"', config)
        self.assertIn('"path": "python/src/cua_driver/__init__.py"', config)
        self.assertIn('"path": "scripts/_install-rust.sh"', config)
        self.assertIn('"path": "scripts/install.ps1"', config)

    def test_installers_preserve_legacy_telemetry_state_before_cleanup(self) -> None:
        for relative_path in (
            "libs/cua-driver/scripts/_install-rust.sh",
            "libs/cua-driver/scripts/_install-local-rust.sh",
        ):
            installer = self.read(relative_path)
            cleanup = installer.index('rm -rf "$LEGACY_HOME_DIR"')
            self.assertLess(
                installer.index("for telemetry_file in .telemetry_id .installation_recorded"),
                cleanup,
                relative_path,
            )
            self.assertLess(
                installer.index('cp -p "$LEGACY_HOME_DIR/$telemetry_file"'),
                cleanup,
                relative_path,
            )

        powershell = self.read("libs/cua-driver/scripts/install.ps1")
        cleanup = powershell.index("Remove-Item -LiteralPath $LegacyHomeDir -Recurse -Force")
        self.assertLess(
            powershell.index(
                "foreach ($telemetryFile in @('.telemetry_id', '.installation_recorded'))"
            ),
            cleanup,
        )
        self.assertLess(
            powershell.index("Copy-Item -LiteralPath $legacyTelemetryPath"),
            cleanup,
        )

    def test_local_macos_signing_uses_an_unambiguous_identity_hash(self) -> None:
        installer = self.read("libs/cua-driver/scripts/_install-local-rust.sh")

        self.assertIn(
            'security find-identity -v -p codesigning "$kc"',
            installer,
        )
        self.assertIn('SIGN_ID="$(ensure_local_signing_identity)"', installer)
        self.assertIn(
            'codesign_bounded 20 --force --deep --sign "$SIGN_ID" "$APP_STAGE"',
            installer,
        )
        self.assertNotIn("printf '%s' \"$CUA_LOCAL_SIGN_CN\"; return", installer)

    def test_release_installers_persist_channel_before_binary_swap(self) -> None:
        shell = self.read("libs/cua-driver/scripts/_install-rust.sh")
        hint = shell.index('> "$HOME_DIR/.telemetry_install_channel"')
        self.assertLess(hint, shell.index('ditto "$SRC_APP" "$APP_DEST"'))
        self.assertLess(hint, shell.index('mv -Tf "$TMP_LINK" "$CURRENT_LINK"'))

        powershell = self.read("libs/cua-driver/scripts/install.ps1")
        hint = powershell.index("Set-Content -LiteralPath $telemetryHintPath")
        self.assertLess(hint, powershell.index("Ensure-Junction $CurrentDir    $versionedDir"))

    def test_release_installers_gate_channel_hint_on_effective_consent(self) -> None:
        shell = self.read("libs/cua-driver/scripts/_install-rust.sh")
        self.assertIn(
            "for telemetry_env_name in CUA_DRIVER_RS_TELEMETRY_ENABLED CUA_TELEMETRY_ENABLED",
            shell,
        )
        self.assertIn('[[ "$TELEMETRY_HINT_FROM_ENV" == "0"', shell)
        self.assertIn('"telemetry_enabled"', shell)
        self.assertIn('[[ "$TELEMETRY_HINT_ENABLED" == "1" ]]', shell)

        powershell = self.read("libs/cua-driver/scripts/install.ps1")
        self.assertIn(
            "@('CUA_DRIVER_RS_TELEMETRY_ENABLED', 'CUA_TELEMETRY_ENABLED')",
            powershell,
        )
        self.assertIn("Properties['telemetry_enabled']", powershell)
        self.assertIn("if ($telemetryHintEnabled)", powershell)

    def test_release_and_skill_installers_do_not_depend_on_github_latest(self) -> None:
        workflow = self.read(".github/workflows/cd-rust-cua-driver.yml")
        self.assertIn("--prerelease", workflow)
        self.assertNotIn("softprops/action-gh-release", workflow)
        self.assertNotIn("bake version into install scripts", workflow.lower())

        windows_skill = self.read("libs/cua-driver/rust/Skills/cua-driver/WINDOWS.md")
        self.assertIn("https://cua.ai/driver/install.ps1", windows_skill)
        self.assertNotIn("/releases/latest/download/install.ps1", windows_skill)

    def test_lume_uses_the_same_draft_finalizer(self) -> None:
        workflow = self.read(".github/workflows/cd-swift-lume.yml")

        self.assertIn("--make-latest", workflow)
        self.assertIn("github_release.py", workflow)
        self.assertNotIn("softprops/action-gh-release", workflow)
        self.assertNotIn("bake-lume-version", workflow)

    def test_lifecycle_telemetry_runs_outside_foreground_command(self) -> None:
        main = self.read("libs/cua-driver/rust/crates/cua-driver/src/main.rs")
        telemetry = self.read("libs/cua-driver/rust/crates/cua-driver/src/telemetry.rs")

        self.assertNotIn("telemetry::ensure_first_run_registration();", main)
        self.assertGreaterEqual(main.count("telemetry::run_lifecycle_worker_if_requested()"), 2)
        self.assertGreaterEqual(main.count("telemetry::spawn_first_run_registration_worker()"), 3)
        self.assertIn("CUA_DRIVER_LIFECYCLE_TELEMETRY_WORKER", telemetry)
        self.assertIn(".stdin(Stdio::null())", telemetry)
        self.assertIn(".stdout(Stdio::null())", telemetry)
        self.assertIn(".stderr(Stdio::null())", telemetry)

    def test_released_linux_smoke_waits_for_assets_and_installs_xkbcommon(self) -> None:
        workflow = self.read(".github/workflows/ci-distro-compat-cua-driver.yml")

        self.assertIn("for attempt in $(seq 1 90)", workflow)
        self.assertIn("release asset was still unavailable after 15 minutes", workflow)
        self.assertEqual(workflow.count("libxkbcommon0"), 4)
        self.assertEqual(workflow.count('libxkbcommon"'), 2)


if __name__ == "__main__":
    unittest.main()

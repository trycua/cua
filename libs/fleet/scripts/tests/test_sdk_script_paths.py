"""SDK script relocation tests; no native builds or live service calls."""

from pathlib import Path
import os
import runpy
import shutil
import subprocess
import tempfile
import unittest


SCRIPTS = Path(__file__).resolve().parents[1]
SYMBOLS = (
    "AccessTokenProvider", "connectWithAccessTokenProvider", "connectWithAccessToken",
    "connectBrowserWithAccessToken", "creationTimestamp", "listNamespaces",
    "listUserApiKeys", "createUserApiKey", "deleteUserApiKey",
)


class SdkScriptPathsTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="fleet script paths ")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.workspace = self.root / "relocated workspace"
        self.scripts = self.workspace / "scripts"
        self.scripts.mkdir(parents=True)
        for pattern in ("*.sh", "*.py"):
            for source in SCRIPTS.glob(pattern):
                shutil.copy2(source, self.scripts / source.name)
        self.binding = self.workspace / "sdk-bindings/ts-uniffi-browser/ts/fleet_sdk.ts"
        self.binding.parent.mkdir(parents=True)
        self.binding.write_text("\n".join(SYMBOLS), encoding="utf-8")
        self.cwd = self.root / "unrelated cwd"
        self.cwd.mkdir()

    def run_script(self, name, *args, env=None):
        return subprocess.run(
            ["bash", str(self.scripts / name), *args], cwd=self.cwd,
            env=env, capture_output=True, text=True, timeout=10,
        )

    def test_browser_contract_follows_relocated_workspace(self):
        result = self.run_script("test-browser-sdk-bindings.sh")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_browser_contract_still_rejects_missing_symbol(self):
        self.binding.write_text("\n".join(SYMBOLS[:-1]), encoding="utf-8")
        result = self.run_script("test-browser-sdk-bindings.sh")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("missing deleteUserApiKey", result.stderr)
        self.assertIn(str(self.binding), result.stderr)
        self.assertNotIn("No such file", result.stderr)

    def test_browser_contract_still_rejects_forbidden_export(self):
        with self.binding.open("a", encoding="utf-8") as binding:
            binding.write("\nexecuteAuthenticated\n")
        result = self.run_script("test-browser-sdk-bindings.sh")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("exports executeAuthenticated", result.stderr)

    def test_native_builder_and_language_launchers_use_workspace_manifest(self):
        # Stop at Cargo deliberately. This tests routing, not generated bindings
        # or a successful native build; no fabricated library is supplied.
        fake_bin = self.root / "fake-bin"
        fake_bin.mkdir()
        cargo = fake_bin / "cargo"
        cargo.write_text("#!/bin/sh\nprintf '%s\\n' \"$@\"\nexit 42\n", encoding="utf-8")
        cargo.chmod(0o755)
        env = {**os.environ, "PATH": f"{fake_bin}{os.pathsep}{os.environ['PATH']}"}
        for script in (
            "build-sdk-bindings-native.sh", "run-python-sdk-binding.sh",
            "run-ruby-sdk-binding.sh", "run-swift-sdk-binding.sh",
        ):
            with self.subTest(script=script):
                result = self.run_script(script, "unused-fixture", env=env)
                self.assertEqual(result.returncode, 42, result.stderr)
                self.assertIn(str(self.workspace / "Cargo.toml"), result.stderr.splitlines())
                self.assertNotIn("cyclops-cs", result.stderr)

    def test_python_build_helpers_follow_relocated_workspace(self):
        wheel = runpy.run_path(str(self.scripts / "build-python-sdk-wheel.py"))
        locate_workspace = wheel["workspace_root"] if "workspace_root" in wheel else wheel["repository_root"]
        self.assertEqual(locate_workspace(), self.workspace.resolve())
        normalizer = runpy.run_path(str(self.scripts / "normalize-compat-sdk-bindings.py"))
        self.assertEqual(normalizer["GO_OUTPUT"], self.workspace.resolve() /
                         "sdk-bindings/go-uniffi/cyclops_sdk_schema/cyclops_sdk_schema.go")
        self.assertEqual(normalizer["NODE_OUTPUT"], self.workspace.resolve() /
                         "sdk-bindings/ts-uniffi/cyclops_sdk_schema.ts")

    def test_shell_entrypoints_do_not_reintroduce_renamed_directory(self):
        for source in SCRIPTS.glob("*.sh"):
            with self.subTest(script=source.name):
                self.assertNotIn("/cyclops-cs", source.read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()

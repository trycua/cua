import importlib.util
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

PROJECT_ROOT = Path(__file__).resolve().parents[1]
BUILD_HOOK_PATH = PROJECT_ROOT / "hatch_build.py"


def load_build_hook():
    spec = importlib.util.spec_from_file_location("cua_sandbox_hatch_build", BUILD_HOOK_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.CustomBuildHook


class CyclopsSdkPackagingTests(unittest.TestCase):
    def test_build_hook_stages_mirrored_cyclops_sdk(self):
        CustomBuildHook = load_build_hook()
        with tempfile.TemporaryDirectory() as temporary_directory:
            temporary_root = Path(temporary_directory)
            repository_root = temporary_root / "repository"
            mirror_root = repository_root / "libs" / "fleet"
            binding_root = mirror_root / "sdk-bindings" / "python" / "cyclops_sdk"
            binding_root.mkdir(parents=True)
            native_library = temporary_root / "native" / "debug" / "libcyclops_sdk.so"
            calls = []

            def fake_run(command, check):
                calls.append((command, check))
                native_library.parent.mkdir(parents=True)
                native_library.touch()

            with (
                patch("platform.system", return_value="Linux"),
                patch("subprocess.run", fake_run),
                patch.object(CustomBuildHook, "repository_root", repository_root),
                patch.object(CustomBuildHook, "native_target_dir", temporary_root / "native"),
            ):
                hook = object.__new__(CustomBuildHook)
                build_data = {"force_include": {}}
                hook.initialize("0.1.0", build_data)

            self.assertEqual(
                calls,
                [
                    (
                        [
                            "cargo",
                            "build",
                            "--locked",
                            "--manifest-path",
                            str(mirror_root / "Cargo.toml"),
                            "--package",
                            "cyclops-sdk",
                            "--target-dir",
                            str(temporary_root / "native"),
                        ],
                        True,
                    )
                ],
            )
            self.assertEqual(
                build_data["force_include"],
                {
                    str(binding_root): "cyclops_sdk",
                    str(native_library): "cyclops_sdk/libcyclops_sdk.so",
                },
            )
            self.assertTrue(build_data["infer_tag"])


if __name__ == "__main__":
    unittest.main()

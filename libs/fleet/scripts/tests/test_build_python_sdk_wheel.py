import csv
import importlib.util
import subprocess
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch


SCRIPT = Path(__file__).resolve().parents[1] / "build-python-sdk-wheel.py"
CYCLOPS_ROOT = Path(__file__).resolve().parents[2]


def load_builder():
    spec = importlib.util.spec_from_file_location("build_python_sdk_wheel", SCRIPT)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class PythonSdkWheelBuildTest(unittest.TestCase):
    def test_windows_x86_64_wheel_contains_only_generated_sdk_and_dll(self):
        builder = load_builder()
        with (
            patch.object(builder.sys, "platform", "win32"),
            patch.object(builder.platform, "machine", return_value="AMD64"),
        ):
            self.assertEqual("cyclops_sdk.dll", builder.native_library_name())
            self.assertEqual("win_amd64", builder.default_platform_tag())

        with tempfile.TemporaryDirectory() as temporary_directory:
            temporary = Path(temporary_directory)
            native_library = temporary / "cyclops_sdk.dll"
            native_library.write_bytes(b"not a native library")
            output = temporary / "dist"

            subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "--version",
                    "0.1.0",
                    "--native-library",
                    str(native_library),
                    "--platform-tag",
                    "win_amd64",
                    "--outdir",
                    str(output),
                ],
                cwd=CYCLOPS_ROOT,
                check=True,
            )

            wheels = list(output.glob("*.whl"))
            self.assertEqual(
                [wheel.name for wheel in wheels],
                ["cua_fleet-0.1.0-py3-none-win_amd64.whl"],
            )
            with zipfile.ZipFile(wheels[0]) as archive:
                names = archive.namelist()
                roots = {name.split("/", 1)[0] for name in names}
                self.assertEqual(roots, {"fleet_sdk", "cua_fleet-0.1.0.dist-info"})
                self.assertIn("fleet_sdk/cyclops_sdk.dll", names)
                self.assertNotIn("cua_train/__init__.py", names)
                self.assertNotIn("cua_fleet/__init__.py", names)
                metadata = archive.read("cua_fleet-0.1.0.dist-info/METADATA").decode()
                self.assertIn("Name: cua-fleet", metadata)
                self.assertIn("Version: 0.1.0", metadata)
                self.assertNotIn("Requires-Dist:", metadata)
                wheel_metadata = archive.read("cua_fleet-0.1.0.dist-info/WHEEL").decode()
                self.assertIn("Root-Is-Purelib: false", wheel_metadata)
                self.assertIn("Tag: py3-none-win_amd64", wheel_metadata)
                record_rows = list(
                    csv.reader(
                        archive.read("cua_fleet-0.1.0.dist-info/RECORD")
                        .decode()
                        .splitlines()
                    )
                )
                self.assertIn(
                    [
                        "fleet_sdk/cyclops_sdk.dll",
                        "sha256=U9PN3aRS1FqC5Fx0fZQjZf4JXxMTFG6yOsDsB5DOjD0",
                        "20",
                    ],
                    record_rows,
                )

    def test_linux_wheel_contains_checked_in_binding_and_native_library(self):
        with tempfile.TemporaryDirectory() as temporary_directory:
            temporary = Path(temporary_directory)
            native_library = temporary / "libcyclops_sdk.so"
            native_library.write_bytes(b"not a native library")
            output = temporary / "dist"

            subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "--version",
                    "0.1.0",
                    "--native-library",
                    str(native_library),
                    "--platform-tag",
                    "linux_x86_64",
                    "--outdir",
                    str(output),
                ],
                cwd=CYCLOPS_ROOT,
                check=True,
            )

            wheel = next(output.glob("*.whl"))
            self.assertEqual(wheel.name, "cua_fleet-0.1.0-py3-none-linux_x86_64.whl")
            with zipfile.ZipFile(wheel) as archive:
                names = archive.namelist()
                self.assertIn("fleet_sdk/__init__.py", names)
                self.assertIn("fleet_sdk/_sdk.py", names)
                self.assertIn("fleet_sdk/_schema.py", names)
                self.assertIn("fleet_sdk/libcyclops_sdk.so", names)

    def test_rejects_native_library_name_the_generated_loader_cannot_find(self):
        with tempfile.TemporaryDirectory() as temporary_directory:
            temporary = Path(temporary_directory)
            native_library = temporary / "libcyclops_sdk.so"
            native_library.write_bytes(b"not a native library")

            process = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "--version",
                    "0.1.0",
                    "--native-library",
                    str(native_library),
                    "--platform-tag",
                    "win_amd64",
                    "--outdir",
                    str(temporary / "dist"),
                ],
                cwd=CYCLOPS_ROOT,
                text=True,
                capture_output=True,
            )

            self.assertNotEqual(0, process.returncode)
            self.assertIn("native library must be named cyclops_sdk.dll", process.stderr)


if __name__ == "__main__":
    unittest.main()

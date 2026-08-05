import csv
import importlib.util
import subprocess
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch


class WheelBuildTest(unittest.TestCase):
    def test_windows_x86_64_wheel_contains_generated_loader_dll_and_platform_tag(self):
        project = Path(__file__).resolve().parents[1]
        builder_path = project / "scripts" / "build_wheel.py"
        spec = importlib.util.spec_from_file_location("build_wheel", builder_path)
        assert spec and spec.loader
        build_wheel = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(build_wheel)

        with (
            patch.object(build_wheel.sys, "platform", "win32"),
            patch.object(build_wheel.platform, "machine", return_value="AMD64"),
        ):
            self.assertEqual("cyclops_sdk.dll", build_wheel.native_library_name())
            self.assertEqual("win_amd64", build_wheel.default_platform_tag())
            self.assertEqual("cyclops_sdk.dll", build_wheel.native_library_name_for_platform_tag("win_amd64"))

        with tempfile.TemporaryDirectory() as temporary_directory:
            temporary = Path(temporary_directory)
            native_library = temporary / "cyclops_sdk.dll"
            native_library.write_bytes(b"not a native library")
            output = temporary / "dist"

            subprocess.run(
                [
                    sys.executable,
                    "scripts/build_wheel.py",
                    "--native-library",
                    str(native_library),
                    "--platform-tag",
                    "win_amd64",
                    "--outdir",
                    str(output),
                ],
                cwd=project,
                check=True,
            )

            wheels = list(output.glob("*.whl"))
            self.assertEqual([wheel.name for wheel in wheels], ["cua_train-0.1.6-py3-none-win_amd64.whl"])
            with zipfile.ZipFile(wheels[0]) as archive:
                names = archive.namelist()
                self.assertIn("fleet_sdk/cyclops_sdk.dll", names)
                wheel_metadata = next(name for name in names if name.endswith(".dist-info/WHEEL"))
                self.assertIn("Root-Is-Purelib: false", archive.read(wheel_metadata).decode())
                self.assertIn("Tag: py3-none-win_amd64", archive.read(wheel_metadata).decode())
                record = next(name for name in names if name.endswith(".dist-info/RECORD"))
                record_rows = list(csv.reader(archive.read(record).decode().splitlines()))
                self.assertIn(
                    ["fleet_sdk/cyclops_sdk.dll", "sha256=U9PN3aRS1FqC5Fx0fZQjZf4JXxMTFG6yOsDsB5DOjD0", "20"],
                    record_rows,
                )

    def test_wheel_build_rejects_a_native_library_name_the_loader_cannot_find(self):
        project = Path(__file__).resolve().parents[1]
        with tempfile.TemporaryDirectory() as temporary_directory:
            temporary = Path(temporary_directory)
            native_library = temporary / "libcyclops_sdk.so"
            native_library.write_bytes(b"not a native library")

            process = subprocess.run(
                [
                    sys.executable,
                    "scripts/build_wheel.py",
                    "--native-library",
                    str(native_library),
                    "--platform-tag",
                    "win_amd64",
                    "--outdir",
                    str(temporary / "dist"),
                ],
                cwd=project,
                text=True,
                capture_output=True,
            )

            self.assertNotEqual(0, process.returncode)
            self.assertIn("native library must be named cyclops_sdk.dll", process.stderr)

    def test_staged_wheel_contains_native_binding_and_platform_tag(self):
        project = Path(__file__).resolve().parents[1]
        with tempfile.TemporaryDirectory() as temporary_directory:
            temporary = Path(temporary_directory)
            native_library = temporary / "libcyclops_sdk.so"
            native_library.write_bytes(b"not a native library")
            output = temporary / "dist"

            subprocess.run(
                [
                    sys.executable,
                    "scripts/build_wheel.py",
                    "--native-library",
                    str(native_library),
                    "--platform-tag",
                    "linux_x86_64",
                    "--outdir",
                    str(output),
                ],
                cwd=project,
                check=True,
            )

            wheels = list(output.glob("*.whl"))
            self.assertEqual([wheel.name for wheel in wheels], ["cua_train-0.1.6-py3-none-linux_x86_64.whl"])
            with zipfile.ZipFile(wheels[0]) as archive:
                names = archive.namelist()
                self.assertIn("fleet_sdk/__init__.py", names)
                self.assertIn("fleet_sdk/_sdk.py", names)
                self.assertIn("fleet_sdk/_schema.py", names)
                self.assertIn("fleet_sdk/libcyclops_sdk.so", names)
                self.assertIn("cua_train/__init__.py", names)
                wheel_metadata = next(name for name in names if name.endswith(".dist-info/WHEEL"))
                self.assertIn("Root-Is-Purelib: false", archive.read(wheel_metadata).decode())
                self.assertIn("Tag: py3-none-linux_x86_64", archive.read(wheel_metadata).decode())
                record = next(name for name in names if name.endswith(".dist-info/RECORD"))
                record_rows = list(csv.reader(archive.read(record).decode().splitlines()))
                self.assertIn(
                    ["fleet_sdk/libcyclops_sdk.so", "sha256=U9PN3aRS1FqC5Fx0fZQjZf4JXxMTFG6yOsDsB5DOjD0", "20"],
                    record_rows,
                )


if __name__ == "__main__":
    unittest.main()

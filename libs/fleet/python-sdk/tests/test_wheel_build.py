import csv
import subprocess
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path


class WheelBuildTest(unittest.TestCase):
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
                    "test_platform",
                    "--outdir",
                    str(output),
                ],
                cwd=project,
                check=True,
            )

            wheels = list(output.glob("*.whl"))
            self.assertEqual([wheel.name for wheel in wheels], ["cua_train-0.1.1-py3-none-test_platform.whl"])
            with zipfile.ZipFile(wheels[0]) as archive:
                names = archive.namelist()
                self.assertIn("cyclops_sdk/__init__.py", names)
                self.assertIn("cyclops_sdk/_sdk.py", names)
                self.assertIn("cyclops_sdk/_schema.py", names)
                self.assertIn("cyclops_sdk/libcyclops_sdk.so", names)
                self.assertIn("cua_train/__init__.py", names)
                wheel_metadata = next(name for name in names if name.endswith(".dist-info/WHEEL"))
                self.assertIn("Root-Is-Purelib: false", archive.read(wheel_metadata).decode())
                self.assertIn("Tag: py3-none-test_platform", archive.read(wheel_metadata).decode())
                record = next(name for name in names if name.endswith(".dist-info/RECORD"))
                record_rows = list(csv.reader(archive.read(record).decode().splitlines()))
                self.assertIn(
                    ["cyclops_sdk/libcyclops_sdk.so", "sha256=U9PN3aRS1FqC5Fx0fZQjZf4JXxMTFG6yOsDsB5DOjD0", "20"],
                    record_rows,
                )


if __name__ == "__main__":
    unittest.main()

from __future__ import annotations

import base64
import csv
import hashlib
import importlib.util
import json
from io import StringIO
from pathlib import Path
import tempfile
import unittest
import zipfile


SCRIPT = Path(__file__).resolve().parents[1] / "verify_cua_fleet_wheels.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_cua_fleet_wheels", SCRIPT)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def record(data: bytes) -> tuple[str, str]:
    digest = base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=").decode()
    return f"sha256={digest}", str(len(data))


def csv_bytes(rows: list[list[str]]) -> bytes:
    output = StringIO()
    csv.writer(output, lineterminator="\n").writerows(rows)
    return output.getvalue().encode()


def write_wheel(path: Path, version: str, native_name: str, legacy: bool = False) -> None:
    dist_info = f"cua_fleet-{version}.dist-info"
    entries = {
        "fleet_sdk/__init__.py": b"CyclopsClient = object\n",
        f"fleet_sdk/{native_name}": b"native payload",
        f"{dist_info}/METADATA": (
            f"Metadata-Version: 2.4\nName: cua-fleet\nVersion: {version}\n\n"
        ).encode(),
        f"{dist_info}/WHEEL": (
            "Wheel-Version: 1.0\nRoot-Is-Purelib: false\n"
            f"Tag: py3-none-{path.name.removesuffix('.whl').split('-py3-none-', 1)[1]}\n"
        ).encode(),
    }
    if legacy:
        entries["cua_train/__init__.py"] = b"TrainClient = object\n"
    record_name = f"{dist_info}/RECORD"
    rows = [[name, *record(data)] for name, data in sorted(entries.items())]
    rows.append([record_name, "", ""])
    entries[record_name] = csv_bytes(rows)
    with zipfile.ZipFile(path, "w") as archive:
        for name, data in entries.items():
            archive.writestr(name, data)


class VerifyCuaFleetWheelsTest(unittest.TestCase):
    def test_verifies_exact_five_platform_manifest_without_mutating_wheels(self):
        verifier = load_verifier()
        with tempfile.TemporaryDirectory() as temporary_directory:
            dist = Path(temporary_directory)
            version = "0.0.11"
            platforms = {
                "manylinux_2_34_x86_64": "libcyclops_sdk.so",
                "manylinux_2_34_aarch64": "libcyclops_sdk.so",
                "macosx_10_12_x86_64": "libcyclops_sdk.dylib",
                "macosx_11_0_arm64": "libcyclops_sdk.dylib",
                "win_amd64": "cyclops_sdk.dll",
            }
            for platform, native_name in platforms.items():
                write_wheel(
                    dist / f"cua_fleet-{version}-py3-none-{platform}.whl",
                    version,
                    native_name,
                )
            before = {path.name: path.read_bytes() for path in dist.glob("*.whl")}
            manifest = {
                "version": version,
                "wheels": {
                    name: hashlib.sha256(data).hexdigest() for name, data in before.items()
                },
            }
            manifest_path = dist / "canonical-wheels.json"
            manifest_path.write_text(json.dumps(manifest))

            verifier.verify_release(manifest_path, dist)

            after = {path.name: path.read_bytes() for path in dist.glob("*.whl")}
            self.assertEqual(before, after)

    def test_rejects_hash_mismatch_and_legacy_payload(self):
        verifier = load_verifier()
        with tempfile.TemporaryDirectory() as temporary_directory:
            dist = Path(temporary_directory)
            wheel = dist / "cua_fleet-0.0.11-py3-none-win_amd64.whl"
            write_wheel(wheel, "0.0.11", "cyclops_sdk.dll", legacy=True)
            manifest = {
                "version": "0.0.11",
                "wheels": {wheel.name: "0" * 64},
            }
            manifest_path = dist / "canonical-wheels.json"
            manifest_path.write_text(json.dumps(manifest))

            with self.assertRaisesRegex(verifier.WheelError, "SHA-256 mismatch"):
                verifier.verify_release(manifest_path, dist, require_all_platforms=False)

            manifest["wheels"][wheel.name] = hashlib.sha256(wheel.read_bytes()).hexdigest()
            manifest_path.write_text(json.dumps(manifest))
            with self.assertRaisesRegex(verifier.WheelError, "unexpected payload roots"):
                verifier.verify_release(manifest_path, dist, require_all_platforms=False)

    def test_allows_one_manifest_wheel_in_a_platform_job(self):
        verifier = load_verifier()
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            dist = root / "dist"
            dist.mkdir()
            version = "0.1.6"
            wheels = {}
            for platform, native_name in verifier.PLATFORM_NATIVE_NAMES.items():
                wheel = root / f"cua_fleet-{version}-py3-none-{platform}.whl"
                write_wheel(wheel, version, native_name)
                wheels[wheel.name] = hashlib.sha256(wheel.read_bytes()).hexdigest()

            selected = root / next(iter(wheels))
            selected.rename(dist / selected.name)
            manifest_path = root / "canonical-wheels.json"
            manifest_path.write_text(json.dumps({"version": version, "wheels": wheels}))

            verifier.verify_release(manifest_path, dist, require_all_platforms=False)


if __name__ == "__main__":
    unittest.main()

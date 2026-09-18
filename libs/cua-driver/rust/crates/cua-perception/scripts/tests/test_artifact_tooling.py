from __future__ import annotations

import json
import io
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest import mock

SCRIPT_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPT_DIR))

from artifact_tooling import (  # noqa: E402
    ArtifactError,
    CRATE_DIR,
    LOCK_PATH,
    binary_target,
    sha256,
    static_verify,
    verify_corresponding_sources,
)
from assemble_bundle import immutable_url  # noqa: E402
from verify_bundle import source_inspection  # noqa: E402


def write(path: Path, contents: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(contents)


def linux_header(payload: bytes = b"") -> bytes:
    header = bytearray(64)
    header[:4] = b"\x7fELF"
    header[4] = 2
    header[5] = 1
    header[18:20] = (62).to_bytes(2, "little")
    return bytes(header) + payload


def windows_header() -> bytes:
    header = bytearray(72)
    header[:2] = b"MZ"
    header[0x3C:0x40] = (64).to_bytes(4, "little")
    header[64:68] = b"PE\0\0"
    header[68:70] = (0x8664).to_bytes(2, "little")
    return bytes(header)


def macos_arm64_header() -> bytes:
    return b"\xcf\xfa\xed\xfe" + (0x0100000C).to_bytes(4, "little") + b"\0" * 56


class ArtifactToolingTests(unittest.TestCase):
    def test_conversion_recipe_pins_input_exporter_sources_command_and_no_patches(self) -> None:
        lock = json.loads(LOCK_PATH.read_text())
        recipe = json.loads((CRATE_DIR / "models/conversion-recipe.json").read_text())
        materials = lock["corresponding_source"]
        source_input = next(item for item in materials if item["content_kind"] == "model-source-input")
        exporters = [item for item in materials if item["content_kind"] == "exporter-source"]
        self.assertEqual(source_input["sha256"], recipe["input"]["sha256"])
        self.assertEqual(source_input["size"], recipe["input"]["size"])
        self.assertEqual(recipe["patches"], [])
        self.assertEqual(recipe["command"][1], "models/export_omniparser_detector.py")
        self.assertEqual(
            {item["filename"].split("-", 1)[0] for item in exporters},
            {"ultralytics", "pytorch", "torchvision", "onnx", "onnxslim"},
        )
        for item in materials:
            immutable_url(item["url"])
            self.assertEqual(len(item["sha256"]), 64)
            self.assertGreater(item["size"], 0)
        self.assertEqual(recipe["expected_output"]["sha256"], "d8a876bf7f9fb73d7da9432904ade7fa78e092e9a91674e5a2806b45562a9ab2")

    def source_fixture(self, root: Path) -> tuple[dict, dict]:
        archive = root / "source/cua-perception-source.tar.gz"
        archive.parent.mkdir(parents=True)
        with tarfile.open(archive, "w:gz") as package:
            contents = b"recipe\n"
            info = tarfile.TarInfo("cua/libs/cua-driver/rust/crates/cua-perception/models/conversion-recipe.json")
            info.size = len(contents)
            package.addfile(info, io.BytesIO(contents))
        model = root / "source/upstream/omniparser-icon-detect-model.pt"
        write(model, b"exact-source-model")
        artifacts = [
            {"kind": "source", "name": archive.name, "path": archive.relative_to(root).as_posix(), "sha256": sha256(archive), "size": archive.stat().st_size, "license": {"spdx": "MIT"}},
            {"kind": "source", "name": model.name, "path": model.relative_to(root).as_posix(), "sha256": sha256(model), "size": model.stat().st_size, "license": {"spdx": "AGPL-3.0-only"}},
        ]
        ledger = {"schemaVersion": 1, "sources": [
            {"artifact": archive.name, "artifactSha256": sha256(archive), "artifactSize": archive.stat().st_size, "license": "MIT", "revision": "a" * 40, "sourceOfferStatus": "bundled-review-only", "contentKind": "cua-source", "format": "tar.gz", "requiredPaths": ["libs/cua-driver/rust/crates/cua-perception/models/conversion-recipe.json"]},
            {"artifact": model.name, "artifactSha256": sha256(model), "artifactSize": model.stat().st_size, "license": "AGPL-3.0-only", "revision": "b" * 40, "sourceOfferStatus": "bundled-review-only", "contentKind": "model-source-input", "format": "file"},
        ]}
        (root / "source-ledger.json").write_text(json.dumps(ledger), encoding="utf-8")
        manifest = {"artifacts": artifacts, "sourceLedger": "source-ledger.json"}
        lock = {"corresponding_source": [{"filename": model.name, "bundle_path": artifacts[1]["path"], "sha256": sha256(model), "size": model.stat().st_size, "revision": "b" * 40, "content_kind": "model-source-input"}]}
        return manifest, lock

    def test_source_verifier_rejects_missing_and_tampered_source_input(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest, lock = self.source_fixture(root)
            verify_corresponding_sources(root, manifest, lock)
            model = root / "source/upstream/omniparser-icon-detect-model.pt"
            model.write_bytes(b"tampered")
            with self.assertRaisesRegex(ArtifactError, "size mismatch"):
                verify_corresponding_sources(root, manifest, lock)
            model.unlink()
            with self.assertRaisesRegex(ArtifactError, "bundle path is not a regular file"):
                verify_corresponding_sources(root, manifest, lock)

    def test_preinstall_inspection_reports_bundled_source_locations(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest, _ = self.source_fixture(root)
            sources = source_inspection(root, manifest)
            self.assertEqual(
                {item["location"] for item in sources},
                {
                    "source/cua-perception-source.tar.gz",
                    "source/upstream/omniparser-icon-detect-model.pt",
                },
            )
            self.assertTrue(all(item["status"] == "bundled-review-only" for item in sources))

    def test_source_verifier_rejects_ledger_mismatch_and_missing_recipe(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest, lock = self.source_fixture(root)
            ledger_path = root / "source-ledger.json"
            ledger = json.loads(ledger_path.read_text())
            ledger["sources"][1]["artifactSha256"] = "0" * 64
            ledger_path.write_text(json.dumps(ledger))
            with self.assertRaisesRegex(ArtifactError, "ledger hash or size differs"):
                verify_corresponding_sources(root, manifest, lock)
            ledger["sources"][1]["artifactSha256"] = manifest["artifacts"][1]["sha256"]
            ledger["sources"][0]["requiredPaths"] = ["models/missing-recipe.json"]
            ledger_path.write_text(json.dumps(ledger))
            with self.assertRaisesRegex(ArtifactError, "lacks required path"):
                verify_corresponding_sources(root, manifest, lock)

    def test_binary_headers_are_target_specific(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            fixtures = {
                "linux": (linux_header(), "x86_64-unknown-linux-gnu"),
                "windows": (windows_header(), "x86_64-pc-windows-msvc"),
                "macos": (macos_arm64_header(), "aarch64-apple-darwin"),
            }
            for name, (contents, target) in fixtures.items():
                path = root / name
                write(path, contents)
                self.assertEqual(binary_target(path), target)

    def test_only_immutable_approved_download_coordinates_are_accepted(self) -> None:
        immutable_url(
            "https://huggingface.co/PaddlePaddle/PP-OCRv5_mobile_det_onnx/resolve/"
            "e6f4fa85f00e168c862bc462aebca69eef9b3d3d/inference.onnx"
        )
        immutable_url(
            "https://github.com/microsoft/onnxruntime/releases/download/v1.26.0/"
            "onnxruntime-osx-arm64-1.26.0.tgz"
        )
        for url in (
            "http://example.invalid/model.onnx",
            "https://huggingface.co/example/model/resolve/main/model.onnx",
            "https://github.com/microsoft/onnxruntime/releases/latest/download/runtime.zip",
        ):
            with self.assertRaises(ArtifactError):
                immutable_url(url)

    def test_static_verifier_rejects_tampered_runtime(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            bundle = Path(temporary)
            worker = bundle / "cua-perception"
            runtime = bundle / "libonnxruntime.so"
            write(worker, linux_header(b"worker"))
            write(runtime, linux_header(b"runtime"))
            model_files = []
            for name in ("icon.onnx", "ocr-det.onnx", "ocr-rec.onnx", "dict.yml"):
                path = bundle / "models" / name
                write(path, name.encode())
                model_files.append(path)
            model_manifest = {
                "schema_version": 1,
                "identity": {"name": "test", "version": "1", "source_url": "x", "source_revision": "x", "license": "x"},
                "onnx_runtime": {"version": "1.26.0", "library_sha256": sha256(runtime), "intra_threads": 1},
                "detector": {"model": {"path": "models/icon.onnx", "sha256": sha256(model_files[0])}},
                "ocr": {
                    "detector": {"model": {"path": "models/ocr-det.onnx", "sha256": sha256(model_files[1])}},
                    "recognizer": {"model": {"path": "models/ocr-rec.onnx", "sha256": sha256(model_files[2])}},
                    "dictionary": {"path": "models/dict.yml", "sha256": sha256(model_files[3])},
                },
            }
            (bundle / "model-manifest.json").write_text(json.dumps(model_manifest), encoding="utf-8")
            entries = []
            for kind, path in (("worker", worker), ("runtime", runtime)):
                entries.append({"kind": kind, "name": path.name, "path": path.name, "sha256": sha256(path), "size": path.stat().st_size})
            lock_artifacts = {
                "icon-detect": ("icon.onnx", sha256(model_files[0])),
                "ocr-detect": ("ocr-det.onnx", sha256(model_files[1])),
                "ocr-recognize": ("ocr-rec.onnx", sha256(model_files[2])),
                "ocr-dictionary": ("dict.yml", sha256(model_files[3])),
            }
            for role, (name, digest) in lock_artifacts.items():
                model_path = bundle / "models" / name
                entries.append({
                    "kind": "dictionary" if role == "ocr-dictionary" else "model",
                    "role": role, "name": name, "path": "models/" + name,
                    "sha256": digest, "size": model_path.stat().st_size,
                })
            manifest = {"target": {"triple": "x86_64-unknown-linux-gnu"}, "artifacts": entries}
            (bundle / "artifact-manifest.json").write_text(json.dumps(manifest), encoding="utf-8")
            real_read_json = __import__("artifact_tooling").read_json
            synthetic_lock = {
                "onnx_runtime": {"targets": {"x86_64-unknown-linux-gnu": {
                    "filename": runtime.name, "sha256": sha256(runtime), "size": runtime.stat().st_size,
                }}},
                "artifacts": [
                    {"role": role, "filename": name, "sha256": digest}
                    for role, (name, digest) in lock_artifacts.items()
                ],
            }

            def patched_read_json(path: Path):
                return synthetic_lock if path.name == "artifacts.lock.json" else real_read_json(path)

            with mock.patch("artifact_tooling.read_json", side_effect=patched_read_json):
                static_verify(bundle, require_host=False)
                runtime.write_bytes(runtime.read_bytes() + b"tampered")
                with self.assertRaisesRegex(ArtifactError, "size mismatch"):
                    static_verify(bundle, require_host=False)

    def test_wrong_platform_worker_is_rejected_before_execution(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "worker.exe"
            write(path, windows_header())
            self.assertEqual(binary_target(path), "x86_64-pc-windows-msvc")
            self.assertNotEqual(binary_target(path), "x86_64-unknown-linux-gnu")


if __name__ == "__main__":
    unittest.main()

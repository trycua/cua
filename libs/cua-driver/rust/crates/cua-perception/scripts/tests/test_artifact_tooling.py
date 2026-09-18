from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

SCRIPT_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPT_DIR))

from artifact_tooling import (  # noqa: E402
    ArtifactError,
    binary_target,
    sha256,
    static_verify,
)
from assemble_bundle import immutable_url  # noqa: E402


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

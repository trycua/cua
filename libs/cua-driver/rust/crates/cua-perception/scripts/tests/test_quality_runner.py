from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import shutil
import stat
import struct
import subprocess
import sys
import tempfile
import textwrap
import unittest
import zipfile
import zlib
from pathlib import Path
from unittest import mock


SCRIPT = Path(__file__).resolve().parents[1] / "quality_runner.py"
sys.path.insert(0, str(SCRIPT.parent))
SPEC = importlib.util.spec_from_file_location("quality_runner", SCRIPT)
assert SPEC and SPEC.loader
quality_runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(quality_runner)


def write(path: Path, data: bytes) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    return path


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def png(path: Path, width: int = 100, height: int = 80) -> Path:
    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    rows = b"".join(b"\x00" + b"\x00\x00\x00" * width for _ in range(height))
    return write(
        path,
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(rows))
        + chunk(b"IEND", b""),
    )


def executable(path: Path, source: str) -> Path:
    path.write_text(textwrap.dedent(source), encoding="utf-8")
    path.chmod(path.stat().st_mode | stat.S_IXUSR)
    return path


def wheel(path: Path, module_source: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(path, "w") as archive:
        archive.writestr("som/__init__.py", textwrap.dedent(module_source))
        archive.writestr("cua_som-1.0.dist-info/METADATA", "Name: cua-som\nVersion: 1.0\n")
    return path


def bound(root: Path, artifact_id: str, path: Path) -> dict[str, object]:
    return {
        "id": artifact_id,
        "source": path.resolve(),
        "sha256": sha256(path),
        "root": root.resolve(),
        "relative_path": path.relative_to(root).as_posix(),
    }


class QualityRunnerTests(unittest.TestCase):
    def test_python_invocation_disables_site_startup_and_inherited_python_paths(self) -> None:
        execution = {
            "python": Path("/bound/python"),
            "python_home": Path("/bound/python-home"),
            "cache_dir": Path("/bound/cache"),
        }
        with mock.patch.dict(os.environ, {"PYTHONPATH": "/unbound", "PYTHONSTARTUP": "/unbound/startup.py"}):
            command, environment = quality_runner._python_child_invocation(execution, {"import_root": "/bound/wheel"})
        self.assertEqual(command[1:3], ["-S", str(SCRIPT)])
        self.assertNotIn("PYTHONPATH", environment)
        self.assertNotIn("PYTHONSTARTUP", environment)
        self.assertEqual(environment["PYTHONHOME"], "/bound/python-home")

    def test_rust_worker_uses_real_framing_and_one_process_for_cold_and_warm(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            image_path = png(root / "images/screen.png")
            image = {"file": "images/screen.png", "sha256": sha256(image_path), "width": 100, "height": 80}
            worker = executable(
                root / "worker",
                f"""\
                #!{sys.executable}
                import base64, hashlib, json, os, struct, sys
                count = 0
                while True:
                    prefix = sys.stdin.buffer.read(4)
                    if not prefix:
                        break
                    size = struct.unpack('>I', prefix)[0]
                    request = json.loads(sys.stdin.buffer.read(size))
                    image = base64.b64decode(request['params']['image']['data_base64'])
                    count += 1
                    result = {{
                        'protocol': 'cua-perception/1', 'request_id': request['request_id'], 'status': 'ok',
                        'result': {{
                            'capture_id': request['params']['capture_id'],
                            'image': {{'sha256': hashlib.sha256(image).hexdigest(), 'width': 100, 'height': 80}},
                            'coordinate_space': 'image_pixels',
                            'regions': [{{'id': 'text-1', 'kind': 'text', 'bounds': {{'x': 10, 'y': 8, 'width': 20, 'height': 16}}, 'text': 'Save', 'confidence': 0.9}}],
                            'runtime': 'onnx_runtime_cpu',
                            'identity': {{'extension': {{'id': 'cua-perception', 'version': '1.0.0'}}, 'pid': os.getpid(), 'count': count}},
                            'text_geometry': 'axis_aligned_bounds'
                        }}
                    }}
                    payload = json.dumps(result, separators=(',', ':')).encode()
                    sys.stdout.buffer.write(struct.pack('>I', len(payload)) + payload)
                    sys.stdout.buffer.flush()
                """,
            )
            manifest = write(root / "model-manifest.json", b"{}")
            runtime = write(root / "runtime.dylib", b"runtime")
            result = quality_runner._run_rust_image(
                {
                    "worker": worker,
                    "manifest": manifest,
                    "runtime": runtime,
                    "extension_id": "cua-perception",
                    "extension_version": "1.0.0",
                    "timeout": 5.0,
                    "executed_artifacts": [
                        bound(root, "worker", worker),
                        bound(root, "manifest", manifest),
                        bound(root, "runtime", runtime),
                    ],
                },
                image,
                image_path,
            )
            self.assertEqual(result["coordinate_space"]["space"], "source_pixels")
            self.assertEqual(result["regions"][0]["bounds"], {"x": 10.0, "y": 8.0, "width": 20.0, "height": 16.0})
            self.assertEqual([sample["phase"] for sample in result["samples"]], ["cold", "warm"])
            self.assertTrue(all(sample["latency_ms"] > 0 for sample in result["samples"]))
            self.assertTrue(all(sample["sampled_process_peak_rss_bytes"] > 0 for sample in result["samples"]))

    def test_malformed_worker_frame_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            script = executable(
                Path(temporary) / "bad-worker",
                f"""\
                #!{sys.executable}
                import struct, sys
                sys.stdout.buffer.write(struct.pack('>I', 20) + b'{{}}')
                sys.stdout.buffer.flush()
                """,
            )
            process = subprocess.Popen([str(script)], stdout=subprocess.PIPE)
            assert process.stdout is not None
            with self.assertRaisesRegex(quality_runner.RunnerError, "truncated frame"):
                quality_runner._read_frame(process.stdout, 2.0)
            process.stdout.close()
            process.wait(timeout=2)

    def test_python_child_imports_local_som_and_emits_source_pixel_xywh(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            import_root = root / "extracted-wheel"
            (import_root / "som").mkdir(parents=True)
            (import_root / "som/__init__.py").write_text(
                textwrap.dedent("""
                    from types import SimpleNamespace

                    class Box:
                        coordinates = [0.1, 0.2, 0.4, 0.5]

                    class Element:
                        id = 7
                        type = "text"
                        bbox = Box()
                        confidence = 0.75
                        content = "Local"

                    class OmniParser:
                        def __init__(self, model_path, cache_dir, force_device):
                            from pathlib import Path
                            if not Path(model_path).is_file() or not (Path(cache_dir) / "model").is_dir():
                                raise RuntimeError("missing explicit model/cache")
                            self.detector = SimpleNamespace(model=object())
                            self.ocr = SimpleNamespace(reader=object())

                        def parse(self, image, **kwargs):
                            return SimpleNamespace(elements=[Element()])
                    """),
                encoding="utf-8",
            )
            model = write(root / "model.pt", b"model")
            (root / "cache/model").mkdir(parents=True)
            (root / "site-packages").mkdir()
            image = png(root / "screen.png")
            options = {
                "import_root": str(import_root),
                "site_packages": str(root / "site-packages"),
                "model": str(model),
                "cache_dir": str(root / "cache"),
                "force_device": "cpu",
                "box_threshold": 0.3,
                "iou_threshold": 0.1,
                "use_ocr": True,
                "image": str(image),
            }
            process = subprocess.Popen(
                [sys.executable, str(SCRIPT), "--python-child", json.dumps(options, separators=(",", ":"))],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            assert process.stdout is not None
            cold = quality_runner._read_frame(process.stdout, 5.0)
            warm = quality_runner._read_frame(process.stdout, 5.0)
            _, stderr = process.communicate(timeout=5)
            self.assertEqual(process.returncode, 0, stderr.decode())
            self.assertEqual((cold["phase"], warm["phase"]), ("cold", "warm"))
            self.assertEqual(cold["regions"], warm["regions"])
            self.assertEqual(cold["regions"][0]["bounds"], {"x": 10.0, "y": 16.0, "width": 30.000000000000004, "height": 24.0})

    def test_python_child_suppresses_sitecustomize_and_rejects_unbound_som_submodule(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            site_packages = root / "site-packages"
            unbound_som = site_packages / "som"
            unbound_som.mkdir(parents=True)
            (unbound_som / "__init__.py").write_text("", encoding="utf-8")
            (unbound_som / "poison.py").write_text("VALUE = 'unbound'\n", encoding="utf-8")
            marker = root / "sitecustomize-loaded"
            (site_packages / "sitecustomize.py").write_text(
                "import pathlib, som.poison\n"
                f"pathlib.Path({str(marker)!r}).write_text('loaded', encoding='utf-8')\n",
                encoding="utf-8",
            )

            import_root = root / "extracted-wheel"
            (import_root / "som").mkdir(parents=True)
            (import_root / "som/__init__.py").write_text(
                "from som import poison\nclass OmniParser: pass\n",
                encoding="utf-8",
            )
            model = write(root / "model.pt", b"model")
            (root / "cache/model").mkdir(parents=True)
            image = png(root / "screen.png")
            options = {
                "import_root": str(import_root),
                "site_packages": str(site_packages),
                "model": str(model),
                "cache_dir": str(root / "cache"),
                "force_device": "cpu",
                "box_threshold": 0.3,
                "iou_threshold": 0.1,
                "use_ocr": False,
                "image": str(image),
            }
            process = subprocess.Popen(
                [sys.executable, "-S", str(SCRIPT), "--python-child", json.dumps(options, separators=(",", ":"))],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            assert process.stdout is not None
            response = quality_runner._read_frame(process.stdout, 5.0)
            _, stderr = process.communicate(timeout=5)
            self.assertFalse(marker.exists(), stderr.decode())
            self.assertEqual(process.returncode, 1)
            self.assertEqual(response["status"], "error")
            self.assertIn("poison", response["error"])

    def test_python_config_requires_bound_runtime_model_and_easyocr_cache(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            python = root / "python"
            shutil.copy2(sys.executable, python)
            model = write(root / "model.pt", b"model")
            ocr_detector = write(root / "cache/model/detector.pth", b"detector")
            ocr_recognizer = write(root / "cache/model/recognizer.pth", b"recognizer")
            package = wheel(root / "cua_som.whl", "class OmniParser: pass")
            runtime = write(root / "requirements.lock", b"lock")
            for directory in (root / "python-home", root / "site-packages"):
                directory.mkdir(parents=True)

            def artifact(role: str, artifact_id: str, path: Path) -> dict[str, str]:
                return {"role": role, "id": artifact_id, "path": path.relative_to(root).as_posix(), "sha256": sha256(path)}

            config = {
                "schema_version": 1,
                "identity": {
                    "model": {"id": "omniparser", "version": "2", "revision": "revision"},
                    "runtime": {"id": "cpython", "version": "3"},
                    "adapter": {"id": "cua-som", "version": "1"},
                    "reference_machine": {"id": "test", "os": "test", "arch": "test", "cpu": "test", "memory_bytes": 1},
                },
                "bindings": {
                    "artifacts": [
                        artifact("worker", "python", python),
                        artifact("source_model", "omniparser_source", model),
                        artifact("ocr_model", "ocr_detector", ocr_detector),
                        artifact("ocr_model", "ocr_recognizer", ocr_recognizer),
                        artifact("package", "package", package),
                        artifact("dependency_lock", "lock", runtime),
                    ],
                    "package_artifact_id": "package",
                    "installed_artifact_ids": ["python", "package", "lock"],
                },
                "execution": {
                    "python_interpreter_artifact_id": "python",
                    "python_home": "python-home",
                    "site_packages": "site-packages",
                    "model_artifact_id": "omniparser_source",
                    "ocr_model_artifact_ids": ["ocr_detector", "ocr_recognizer"],
                    "cache_dir": "cache",
                    "force_device": "cpu",
                    "box_threshold": 0.3,
                    "iou_threshold": 0.1,
                    "use_ocr": True,
                    "timeout_seconds": 5,
                },
            }
            config_path = root / "config.json"
            config_path.write_text(json.dumps(config), encoding="utf-8")
            loaded = quality_runner._load_config(config_path, "python")
            self.assertEqual(loaded["execution"]["python"], python.resolve())
            self.assertEqual(loaded["execution"]["package"], package.resolve())
            self.assertEqual(loaded["execution"]["ocr_models"], [ocr_detector.resolve(), ocr_recognizer.resolve()])
            package.write_bytes(b"tampered")
            with self.assertRaisesRegex(quality_runner.RunnerError, "changed after configuration validation"):
                with tempfile.TemporaryDirectory() as extraction:
                    quality_runner._extract_bound_wheel(package, loaded["execution"]["package_sha256"], Path(extraction))
            package = wheel(root / "cua_som.whl", "class OmniParser: pass")
            config["bindings"]["artifacts"][4]["sha256"] = sha256(package)
            config["execution"]["source_root"] = "source"
            config_path.write_text(json.dumps(config), encoding="utf-8")
            with self.assertRaisesRegex(quality_runner.RunnerError, "unknown fields: source_root"):
                quality_runner._load_config(config_path, "python")
            config["execution"].pop("source_root")
            (root / "empty-cache").mkdir()
            config["execution"]["cache_dir"] = "empty-cache"
            config_path.write_text(json.dumps(config), encoding="utf-8")
            with self.assertRaisesRegex(quality_runner.RunnerError, "model directory"):
                quality_runner._load_config(config_path, "python")

    def test_rust_config_binds_every_worker_loaded_manifest_artifact(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            worker = executable(root / "worker", "#!/bin/sh\nexit 0\n")
            runtime = write(root / "runtime.dylib", b"runtime")
            source = write(root / "source/model.pt", b"source")
            detector = write(root / "bundle/artifacts/detector.onnx", b"detector")
            ocr_detector = write(root / "bundle/artifacts/ocr-detector.onnx", b"ocr-detector")
            ocr_recognizer = write(root / "bundle/artifacts/ocr-recognizer.onnx", b"ocr-recognizer")
            dictionary = write(root / "bundle/artifacts/dictionary.txt", b"dictionary")
            manifest_value = {
                "identity": {"source_revision": "revision"},
                "onnx_runtime": {"library_sha256": sha256(runtime)},
                "detector": {"model": {"path": "artifacts/detector.onnx", "sha256": sha256(detector)}},
                "ocr": {
                    "detector": {"model": {"path": "artifacts/ocr-detector.onnx", "sha256": sha256(ocr_detector)}},
                    "recognizer": {"model": {"path": "artifacts/ocr-recognizer.onnx", "sha256": sha256(ocr_recognizer)}},
                    "dictionary": {"path": "artifacts/dictionary.txt", "sha256": sha256(dictionary)},
                },
            }
            manifest = root / "bundle/model-manifest.json"
            manifest.write_text(json.dumps(manifest_value), encoding="utf-8")
            conversion_value = {"input": {"sha256": sha256(source)}, "expected_output": {"sha256": sha256(detector)}}
            conversion = root / "conversion.json"
            conversion.write_text(json.dumps(conversion_value), encoding="utf-8")
            package = write(root / "package.tar.gz", b"package")
            lock = write(root / "lock.json", b"lock")

            def artifact(role: str, artifact_id: str, path: Path) -> dict[str, str]:
                return {"role": role, "id": artifact_id, "path": path.relative_to(root).as_posix(), "sha256": sha256(path)}

            artifacts = [
                artifact("worker", "worker", worker), artifact("runtime", "runtime", runtime),
                artifact("source_model", "omniparser_source", source), artifact("converted_model", "detector", detector),
                artifact("ocr_model", "ocr_detector", ocr_detector), artifact("ocr_model", "ocr_recognizer", ocr_recognizer),
                artifact("ocr_dictionary", "dictionary", dictionary), artifact("model_manifest", "manifest", manifest),
                artifact("conversion_metadata", "conversion", conversion), artifact("package", "package", package),
                artifact("dependency_lock", "lock", lock),
            ]
            config = {
                "schema_version": 1,
                "identity": {
                    "model": {"id": "omniparser", "version": "2", "revision": "revision"},
                    "runtime": {"id": "onnx-runtime", "version": "1"},
                    "adapter": {"id": "cua-perception", "version": "1"},
                    "reference_machine": {"id": "test", "os": "test", "arch": "test", "cpu": "test", "memory_bytes": 1},
                },
                "bindings": {"artifacts": artifacts, "package_artifact_id": "package", "installed_artifact_ids": ["worker", "runtime", "detector"]},
                "execution": {
                    "worker_artifact_id": "worker", "manifest_artifact_id": "manifest", "onnx_runtime_artifact_id": "runtime",
                    "source_model_artifact_id": "omniparser_source", "detector_artifact_id": "detector",
                    "ocr_detector_artifact_id": "ocr_detector", "ocr_recognizer_artifact_id": "ocr_recognizer",
                    "ocr_dictionary_artifact_id": "dictionary", "conversion_metadata_artifact_id": "conversion",
                    "extension_id": "cua-perception", "extension_version": "1", "timeout_seconds": 5,
                },
            }
            config_path = root / "config.json"
            config_path.write_text(json.dumps(config), encoding="utf-8")
            loaded = quality_runner._load_config(config_path, "rust")
            self.assertEqual(loaded["execution"]["source_revision"], "revision")

            runtime.write_bytes(b"tampered runtime")
            with self.assertRaisesRegex(quality_runner.RunnerError, "runtime changed after configuration validation"):
                quality_runner._verify_bound_artifacts(loaded["execution"]["executed_artifacts"], "Rust execution")
            runtime.write_bytes(b"runtime")

            alternate = write(root / "alternate.onnx", b"alternate")
            config["bindings"]["artifacts"].append(artifact("converted_model", "alternate", alternate))
            config["execution"]["detector_artifact_id"] = "alternate"
            config_path.write_text(json.dumps(config), encoding="utf-8")
            with self.assertRaisesRegex(quality_runner.RunnerError, "does not match its configured bound artifact"):
                quality_runner._load_config(config_path, "rust")

    def test_closed_corpus_rejects_extra_images_and_symlinks(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest = root / "manifest.json"
            expected = png(root / "images/expected.png")
            corpus = [{"file": "images/expected.png"}]
            quality_runner._validate_closed_corpus(manifest, corpus)
            png(root / "images/extra.png")
            with self.assertRaisesRegex(quality_runner.RunnerError, "extra"):
                quality_runner._validate_closed_corpus(manifest, corpus)
            (root / "images/extra.png").unlink()
            (root / "images/link.png").symlink_to(expected)
            with self.assertRaisesRegex(quality_runner.RunnerError, "symlink"):
                quality_runner._validate_closed_corpus(manifest, corpus)


if __name__ == "__main__":
    unittest.main()

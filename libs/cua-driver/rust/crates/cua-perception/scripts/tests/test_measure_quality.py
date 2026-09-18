from __future__ import annotations

import hashlib
import importlib.util
import json
import struct
import subprocess
import sys
import tempfile
import unittest
import zlib
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "measure_quality.py"
SPEC = importlib.util.spec_from_file_location("measure_quality", SCRIPT)
assert SPEC and SPEC.loader
measure_quality = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(measure_quality)


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write(path: Path, data: bytes) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    return path


def png(path: Path, width: int = 100, height: int = 80) -> None:
    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    rows = b"".join(b"\x00" + b"\x00\x00\x00" * width for _ in range(height))
    write(
        path,
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(rows))
        + chunk(b"IEND", b""),
    )


class QualityFixture:
    def __init__(self, root: Path) -> None:
        self.root = root
        self.image = root / "images/screen.png"
        png(self.image)
        self.manifest_path = root / "manifest.json"
        self.manifest = {
            "schema_version": 1,
            "corpus_version": "test-1",
            "license": "MIT",
            "coordinate_system": {"space": "source_pixels", "origin": "top_left", "bbox_format": "x_y_width_height"},
            "metric_definitions": {
                "text_detection_iou": {"unit": "ratio", "description": "Text IoU."},
                "control_detection_iou": {"unit": "ratio", "description": "Control IoU."},
                "control_kind_accuracy": {"unit": "ratio", "description": "Kind accuracy."},
            },
            "images": [{
                "file": "images/screen.png", "sha256": sha256(self.image), "width": 100, "height": 80,
                "scenario_tags": ["synthetic"],
                "expected": {
                    "text": [
                        {"id": "save-text", "text": "Save", "bbox": [10, 10, 20, 10], "role": "label"},
                        {"id": "cancel-text", "text": "Cancel", "bbox": [50, 10, 30, 10], "role": "label"},
                    ],
                    "controls": [
                        {"id": "save", "kind": "button", "label": "Save", "bbox": [8, 30, 30, 20], "state": "enabled"},
                        {"id": "cancel", "kind": "button", "label": "Cancel", "bbox": [50, 30, 35, 20], "state": "enabled"},
                    ],
                },
            }],
        }
        self.manifest_path.write_text(json.dumps(self.manifest), encoding="utf-8")

    def artifact(self, engine: str, role: str, artifact_id: str, data: bytes) -> dict:
        prefix = "shared" if artifact_id == "omniparser_source" else engine
        path = write(self.root / f"bound/{prefix}/{artifact_id}", data)
        return {"role": role, "id": artifact_id, "path": path.relative_to(self.root).as_posix(), "sha256": sha256(path)}

    def artifacts(self, engine: str) -> list[dict]:
        values = [
            self.artifact(engine, "source_model", "omniparser_source", b"same-upstream-model-pt"),
            self.artifact(engine, "runtime", "runtime", f"{engine}-runtime".encode()),
            self.artifact(engine, "worker", "worker", f"{engine}-worker".encode()),
            self.artifact(engine, "package", "package", f"{engine}-package".encode()),
            self.artifact(engine, "dependency_lock", "dependency_lock", f"{engine}-lock".encode()),
        ]
        if engine == "rust":
            values.extend([
                self.artifact(engine, "converted_model", "omniparser_onnx", b"rust-converted-detector"),
                self.artifact(engine, "ocr_model", "ppocr_detector", b"ppocr-det"),
                self.artifact(engine, "ocr_model", "ppocr_recognizer", b"ppocr-rec"),
                self.artifact(engine, "ocr_dictionary", "ppocr_dictionary", b"ppocr-dict"),
            ])
        else:
            values.extend([
                self.artifact(engine, "ocr_model", "easyocr_detector", b"easyocr-detector"),
                self.artifact(engine, "ocr_model", "easyocr_recognizer", b"easyocr-recognizer"),
            ])
        return values

    def results(self, engine: str) -> dict:
        return {
            "schema_version": 1,
            "engine": engine,
            "identity": {
                "model": {"id": "omniparser", "version": "2", "revision": "upstream-rev"},
                "runtime": {"id": f"{engine}-runtime", "version": "1"},
                "adapter": {"id": f"{engine}-adapter", "version": "1"},
                "reference_machine": {"id": "bench-1", "os": "linux", "arch": "x86_64", "cpu": "test cpu", "memory_bytes": 1024},
            },
            "bindings": {
                "artifacts": self.artifacts(engine),
                "package_artifact_id": "package",
                "installed_artifact_ids": ["worker", "runtime", "omniparser_source"],
            },
            "images": [{
                "file": "images/screen.png", "image_sha256": sha256(self.image),
                "coordinate_space": {"space": "source_pixels", "origin": "top_left", "bbox_format": "x_y_width_height", "width": 100, "height": 80},
                "regions": [
                    {"kind": "text", "bounds": {"x": 12, "y": 10, "width": 20, "height": 10}, "text": " Save ", "confidence": 0.01},
                    {"kind": "text", "bounds": {"x": 11, "y": 10, "width": 20, "height": 10}, "text": "Save", "confidence": 0.99},
                    {"kind": "text", "bounds": {"x": 50, "y": 10, "width": 30, "height": 10}, "text": "cancel"},
                    {"kind": "icon", "bounds": {"x": 10, "y": 31, "width": 30, "height": 20}, "control_kind": "button", "label": "Save", "state": "enabled"},
                    {"kind": "control", "bounds": {"x": 9, "y": 30, "width": 30, "height": 20}, "control_kind": "button", "label": "Save", "state": "enabled"},
                    {"kind": "control", "bounds": {"x": 90, "y": 60, "width": 5, "height": 5}, "control_kind": "button", "label": "Other", "state": "enabled"},
                ],
                "samples": [
                    {"phase": "cold", "latency_ms": 10.0, "sampled_process_peak_rss_bytes": 1000},
                    {"phase": "warm", "latency_ms": 4.0, "sampled_process_peak_rss_bytes": 800},
                ],
            }],
        }

    def write_results(self, engine: str, value: dict | None = None) -> Path:
        path = self.root / f"{engine}.json"
        path.write_text(json.dumps(value or self.results(engine)), encoding="utf-8")
        return path


class MeasureQualityTests(unittest.TestCase):
    def test_metrics_are_one_to_one_duplicate_neutral_and_artifact_backed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fixture = QualityFixture(Path(temporary))
            report = measure_quality.build_report(
                fixture.manifest_path, fixture.write_results("rust"), fixture.write_results("python")
            )
            self.assertEqual(report["thresholds"], {"status": "unset", "values": {}})
            for engine in ("rust", "python"):
                measured = report["engines"][engine]
                quality = measured["quality"]
                self.assertEqual(quality["expected_control_recall"], {"matched": 1, "expected": 2, "ratio": 0.5})
                self.assertEqual(quality["ocr_target_string_recall"], {"matched": 1, "expected": 2, "ratio": 0.5})
                self.assertEqual(quality["false_positives"], {"count": 1, "text": 0, "control": 1})
                self.assertEqual(quality["false_positive_count"], quality["false_positives"])
                self.assertEqual(quality["text_detection_iou"]["count"], 2)
                self.assertEqual(quality["text_recognition_exact"], {"correct": 1, "matched": 2, "ratio": 0.5})
                self.assertEqual(quality["control_detection_iou"]["count"], 1)
                self.assertEqual(quality["control_kind_accuracy"], {
                    "status": "available", "correct": 1, "matched": 1, "evaluated": 1,
                    "coverage": {"available": 1, "eligible": 1, "ratio": 1.0}, "ratio": 1.0,
                })
                self.assertEqual(quality["control_label_accuracy"]["ratio"], 1.0)
                self.assertEqual(quality["control_state_accuracy"]["ratio"], 1.0)
                self.assertEqual(quality["bounding_box_center_error_pixels"]["count"], 3)
                self.assertEqual(measured["performance"]["latency_ms"]["warm"]["median"], 4.0)
                self.assertEqual(measured["performance"]["sampled_process_peak_rss_bytes"]["overall_max"], 1000)
                self.assertNotIn("peak_memory_bytes", measured["performance"])
                package = fixture.root / f"bound/{engine}/package"
                installed = [fixture.root / f"bound/{engine}/worker", fixture.root / f"bound/{engine}/runtime", fixture.root / "bound/shared/omniparser_source"]
                self.assertEqual(measured["footprint"]["artifact_size_bytes"], package.stat().st_size)
                self.assertEqual(measured["footprint"]["bound_installed_artifact_bytes"], sum(path.stat().st_size for path in installed))
                self.assertNotIn("installed_size_bytes", measured["footprint"])
            encoded = measure_quality.canonical_json(report)
            self.assertNotIn(temporary, encoded)
            self.assertNotIn("bound/", encoded)

    def test_control_attribute_accuracy_reports_coverage_and_skips_omitted_fields(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fixture = QualityFixture(Path(temporary))
            for engine in ("rust", "python"):
                results = fixture.results(engine)
                image = results["images"][0]
                image["regions"] = [
                    *[region for region in image["regions"] if region["kind"] == "text"],
                    {"kind": "control", "bounds": {"x": 9, "y": 30, "width": 30, "height": 20}},
                    {
                        "kind": "control",
                        "bounds": {"x": 50, "y": 30, "width": 35, "height": 20},
                        "control_kind": "button",
                        "label": "Wrong",
                        "state": "disabled",
                    },
                ]
                fixture.write_results(engine, results)

            report = measure_quality.build_report(
                fixture.manifest_path, fixture.root / "rust.json", fixture.root / "python.json"
            )
            quality = report["engines"]["rust"]["quality"]
            self.assertEqual(quality["expected_control_recall"], {"matched": 2, "expected": 2, "ratio": 1.0})
            self.assertEqual(quality["control_detection_iou"]["count"], 2)
            self.assertEqual(quality["control_kind_accuracy"], {
                "status": "available", "correct": 1, "matched": 2, "evaluated": 1,
                "coverage": {"available": 1, "eligible": 2, "ratio": 0.5}, "ratio": 1.0,
            })
            self.assertEqual(quality["control_label_accuracy"], {
                "status": "available", "correct": 0, "matched": 2, "evaluated": 1,
                "coverage": {"available": 1, "eligible": 2, "ratio": 0.5}, "ratio": 0.0,
                "expected_labeled": 2,
            })
            self.assertEqual(quality["control_state_accuracy"], {
                "status": "available", "correct": 0, "matched": 2, "evaluated": 1,
                "coverage": {"available": 1, "eligible": 2, "ratio": 0.5}, "ratio": 0.0,
            })

    def test_control_attribute_accuracy_is_unavailable_when_matched_regions_omit_fields(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fixture = QualityFixture(Path(temporary))
            for engine in ("rust", "python"):
                results = fixture.results(engine)
                image = results["images"][0]
                image["regions"] = [
                    *[region for region in image["regions"] if region["kind"] == "text"],
                    {"kind": "control", "bounds": {"x": 9, "y": 30, "width": 30, "height": 20}},
                ]
                fixture.write_results(engine, results)

            report = measure_quality.build_report(
                fixture.manifest_path, fixture.root / "rust.json", fixture.root / "python.json"
            )
            quality = report["engines"]["rust"]["quality"]
            for name in ("control_kind_accuracy", "control_label_accuracy", "control_state_accuracy"):
                metric = quality[name]
                self.assertEqual(metric["status"], "unavailable")
                self.assertEqual(metric["matched"], 1)
                self.assertEqual(metric["evaluated"], 0)
                self.assertEqual(metric["coverage"], {"available": 0, "eligible": 1, "ratio": 0.0})
                self.assertIsNone(metric["ratio"])

    def test_distinct_ocr_conversion_and_runtime_artifacts_are_comparable(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fixture = QualityFixture(Path(temporary))
            report = measure_quality.build_report(
                fixture.manifest_path, fixture.write_results("rust"), fixture.write_results("python")
            )
            rust = report["engines"]["rust"]["identity"]
            python = report["engines"]["python"]["identity"]
            self.assertNotEqual(rust["model"]["artifacts_sha256"], python["model"]["artifacts_sha256"])
            self.assertNotEqual(rust["runtime"]["artifacts_sha256"], python["runtime"]["artifacts_sha256"])

    def test_source_model_hash_or_revision_mismatch_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fixture = QualityFixture(Path(temporary))
            rust = fixture.write_results("rust")
            python_results = fixture.results("python")
            python_results["identity"]["model"]["revision"] = "different-rev"
            python = fixture.write_results("python", python_results)
            with self.assertRaisesRegex(measure_quality.MeasurementError, "same model identity"):
                measure_quality.build_report(fixture.manifest_path, rust, python)

            python_results = fixture.results("python")
            source = next(item for item in python_results["bindings"]["artifacts"] if item["id"] == "omniparser_source")
            alternate = write(fixture.root / "bound/python/alternate-source", b"different-upstream-model")
            source["path"] = alternate.relative_to(fixture.root).as_posix()
            source["sha256"] = sha256(alternate)
            python = fixture.write_results("python", python_results)
            with self.assertRaisesRegex(measure_quality.MeasurementError, "same model identity"):
                measure_quality.build_report(fixture.manifest_path, rust, python)

    def test_adapter_coordinates_are_converted_to_source_pixels(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fixture = QualityFixture(Path(temporary))
            rust_results = fixture.results("rust")
            image = rust_results["images"][0]
            image["coordinate_space"].update({"space": "adapter_pixels", "width": 50, "height": 40})
            for region in image["regions"]:
                for field in ("x", "y", "width", "height"):
                    region["bounds"][field] /= 2
            report = measure_quality.build_report(
                fixture.manifest_path,
                fixture.write_results("rust", rust_results),
                fixture.write_results("python"),
            )
            self.assertEqual(report["engines"]["rust"]["quality"], report["engines"]["python"]["quality"])

    def test_bound_bytes_are_hashed_and_symlinks_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fixture = QualityFixture(Path(temporary))
            corpus = measure_quality.validate_manifest(fixture.manifest, fixture.manifest_path)
            results = fixture.results("rust")
            package = next(item for item in results["bindings"]["artifacts"] if item["id"] == "package")
            (fixture.root / package["path"]).write_bytes(b"tampered")
            with self.assertRaisesRegex(measure_quality.MeasurementError, "does not match the bound file"):
                measure_quality.validate_results(results, "rust", corpus, fixture.root / "rust.json")
            results = fixture.results("rust")
            runtime = next(item for item in results["bindings"]["artifacts"] if item["id"] == "runtime")
            runtime_path = fixture.root / runtime["path"]
            target = fixture.root / "actual-runtime"
            runtime_path.replace(target)
            runtime_path.symlink_to(target)
            with self.assertRaisesRegex(measure_quality.MeasurementError, "must not traverse a symlink"):
                measure_quality.validate_results(results, "rust", corpus, fixture.root / "rust.json")

    def test_closed_structures_path_free_identities_and_exact_roles(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fixture = QualityFixture(Path(temporary))
            corpus = measure_quality.validate_manifest(fixture.manifest, fixture.manifest_path)
            results = fixture.results("rust")
            results["unknown"] = True
            with self.assertRaisesRegex(measure_quality.MeasurementError, "unknown fields"):
                measure_quality.validate_results(results, "rust", corpus, fixture.root / "rust.json")
            results.pop("unknown")
            results["identity"]["model"]["id"] = "/private/model"
            with self.assertRaisesRegex(measure_quality.MeasurementError, "path-free identity"):
                measure_quality.validate_results(results, "rust", corpus, fixture.root / "rust.json")
            results = fixture.results("rust")
            results["bindings"]["artifacts"][0]["role"] = "arbitrary"
            with self.assertRaisesRegex(measure_quality.MeasurementError, "role is unsupported"):
                measure_quality.validate_results(results, "rust", corpus, fixture.root / "rust.json")

    def test_rejects_unsafe_json_coordinates_and_missing_samples(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fixture = QualityFixture(Path(temporary))
            duplicate = fixture.root / "duplicate.json"
            duplicate.write_text('{"a":1,"a":2}', encoding="utf-8")
            with self.assertRaisesRegex(measure_quality.MeasurementError, "duplicate JSON key"):
                measure_quality.read_json(duplicate)
            corpus = measure_quality.validate_manifest(fixture.manifest, fixture.manifest_path)
            results = fixture.results("rust")
            results["images"][0]["coordinate_space"]["width"] = 99
            with self.assertRaisesRegex(measure_quality.MeasurementError, "must match the corpus"):
                measure_quality.validate_results(results, "rust", corpus, fixture.root / "rust.json")
            results = fixture.results("rust")
            results["images"][0]["samples"] = [results["images"][0]["samples"][0]]
            with self.assertRaisesRegex(measure_quality.MeasurementError, "exactly one cold and one warm"):
                measure_quality.validate_results(results, "rust", corpus, fixture.root / "rust.json")
            results = fixture.results("rust")
            results["images"][0]["samples"].append(dict(results["images"][0]["samples"][1]))
            with self.assertRaisesRegex(measure_quality.MeasurementError, "exactly one cold and one warm"):
                measure_quality.validate_results(results, "rust", corpus, fixture.root / "rust.json")

    def test_cli_is_repeatable_and_zero_annotations_have_null_ratios(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fixture = QualityFixture(Path(temporary))
            fixture.manifest["images"][0]["expected"] = {"text": [], "controls": []}
            fixture.manifest_path.write_text(json.dumps(fixture.manifest), encoding="utf-8")
            for engine in ("rust", "python"):
                results = fixture.results(engine)
                results["images"][0]["regions"] = []
                fixture.write_results(engine, results)
            first = fixture.root / "first.json"
            second = fixture.root / "second.json"
            args = [str(SCRIPT), "--manifest", str(fixture.manifest_path), "--rust-results", str(fixture.root / "rust.json"), "--python-results", str(fixture.root / "python.json")]
            subprocess.run([sys.executable, *args, "--output", str(first)], check=True)
            subprocess.run([sys.executable, *args, "--output", str(second)], check=True)
            self.assertEqual(first.read_bytes(), second.read_bytes())
            quality = json.loads(first.read_text())["engines"]["rust"]["quality"]
            self.assertIsNone(quality["expected_control_recall"]["ratio"])
            self.assertIsNone(quality["control_kind_accuracy"]["ratio"])
            self.assertEqual(quality["control_kind_accuracy"]["status"], "unavailable")
            self.assertEqual(quality["text_detection_iou"]["count"], 0)


if __name__ == "__main__":
    unittest.main()

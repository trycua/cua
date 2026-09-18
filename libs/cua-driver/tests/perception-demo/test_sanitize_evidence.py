import importlib.util
import base64
import json
from pathlib import Path
import subprocess
import tempfile
import unittest


HERE = Path(__file__).parent
SPEC = importlib.util.spec_from_file_location("sanitize_evidence", HERE / "sanitize_evidence.py")
sanitizer = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(sanitizer)


class EvidenceSanitizerTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)

    def tearDown(self):
        self.temporary.cleanup()

    def inputs(self):
        extension_status = self.root / "extension-status.json"
        parser_result = self.root / "parser-result.json"
        chooser_result = self.root / "chooser-result.json"
        candidate_measurements = self.root / "candidate-measurements.json"
        driver_binary = self.root / "cua-driver"
        model = self.root / "model.bin"
        oracle = self.root / "oracle.json"
        raw_evidence = self.root / "raw-manifest.json"
        recording = self.root / "recording.mp4"
        extension_status.write_text(json.dumps({
            "id": "cua-perception",
            "display_name": "Cua Perception",
            "description": "fixture",
            "protocol_version": 1,
            "installed": True,
            "active_version": "0.1.0",
            "healthy": True,
            "trust": "review-only-publisher-verified",
            "publisher_id": "cua-review-only",
            "publisher_key_id": "review-only-build-override",
            "catalog_version": 7,
            "detail": "healthy",
        }))
        parser_result.write_text(json.dumps({
            "schema": "cua.visual_regions_v1",
            "parser": {"model_id": "cua-perception/demo-v1"},
            "regions": [{"private": "raw-only"}],
        }))
        chooser_result.write_text(json.dumps({
            "schema": "cua.jev_choice_v1",
            "selected_id": "region:send",
            "model": "mock",
            "confidence": 1.0,
            "probabilities": {"region:send": 1.0, "reobserve": 0.0, "abstain": 0.0},
        }))
        model.write_bytes(b"measured-model-bytes")
        driver_binary.write_bytes(b"measured-driver-bytes")
        public_key = bytes(range(32))
        candidate_measurements.write_text(json.dumps({
            "review_only": True,
            "source_sha": "a" * 40,
            "target": "x86_64-unknown-linux-gnu",
            "supplied_model_asset_id": 1234,
            "supplied_model_sha256": sanitizer.sha256_file(model),
            "supplied_model_size": model.stat().st_size,
            "public_key_base64": base64.b64encode(public_key).decode(),
            "public_key_sha256": sanitizer.hashlib.sha256(public_key).hexdigest(),
            "key_id": "review-only-build-override",
            "publisher_id": "cua-review-only",
            "signature_algorithm": "ed25519",
            "catalog_sha256": "d" * 64,
            "archive_sha256": "b" * 64,
            "review_driver_relative_path": "review-cua-driver",
            "review_driver_build_profile": "debug-review-trust-root",
            "review_driver_sha256": sanitizer.sha256_file(driver_binary),
        }))
        oracle.write_text(json.dumps({
            "fixture": "visual-only-canvas/v1",
            "ready": True,
            "selected": "send",
            "action_count": 1,
        }))
        raw_evidence.write_text(json.dumps({
            "capture_ids": {"acted": "private-capture", "fresh": "fresh-capture"},
            "coordinates": [394.0, 270.0],
        }))
        recording.write_bytes(b"measured-video-bytes")
        return {
            "source_sha": "a" * 40,
            "platform": "linux-x11",
            "jev_source_sha": "e" * 40,
            "session_label": "authorized-jev-choice-demo",
            "os_name": "ubuntu22",
            "os_version": "20260915.1",
            "os_arch": "X64",
            "chooser_mode": "mock",
            "candidate_measurements": candidate_measurements,
            "driver_binary": driver_binary,
            "extension_status": extension_status,
            "parser_result": parser_result,
            "chooser_result": chooser_result,
            "model": model,
            "oracle": oracle,
            "raw_evidence": raw_evidence,
            "recording": recording,
        }

    def test_manifest_is_bound_to_driver_status_and_raw_evidence(self):
        inputs = self.inputs()
        manifest = sanitizer.build_manifest(**inputs)
        perception = manifest["runtime"]["perception"]
        self.assertEqual(manifest["schema"], "cua-visual-perception-demo-evidence/v2")
        self.assertEqual(manifest["raw_evidence_sha256"], sanitizer.sha256_file(inputs["raw_evidence"]))
        self.assertEqual(perception["trust"], "review-only-publisher-verified")
        self.assertEqual(perception["signature_algorithm"], "ed25519")
        self.assertEqual(perception["publisher_key_id"], "review-only-build-override")
        self.assertEqual(perception["catalog_version"], 7)
        self.assertEqual(perception["signed_extension_archive_sha256"], "b" * 64)
        public_key = bytes(range(32))
        self.assertEqual(perception["signing_key_sha256"], sanitizer.hashlib.sha256(public_key).hexdigest())
        self.assertEqual(perception["signed_catalog_sha256"], "d" * 64)
        self.assertEqual(manifest["runtime"]["driver"], {
            "source_sha": "a" * 40,
            "binary_sha256": sanitizer.sha256_file(inputs["driver_binary"]),
        })
        self.assertEqual(manifest["runtime"]["chooser"]["provider"], "fixture")
        self.assertEqual(manifest["runtime"]["chooser"]["model_id"], "mock")
        self.assertEqual(manifest["runtime"]["chooser"]["adapter_source_sha"], "a" * 40)
        self.assertEqual(manifest["runtime"]["chooser"]["source_sha"], "e" * 40)
        self.assertEqual(manifest["observation"], {
            "session_label": "authorized-jev-choice-demo",
            "acted_capture_id_sha256": sanitizer.hashlib.sha256(b"private-capture").hexdigest(),
            "fresh_capture_id_sha256": sanitizer.hashlib.sha256(b"fresh-capture").hexdigest(),
        })
        self.assertEqual(manifest["os"], {
            "name": "ubuntu22", "version": "20260915.1", "arch": "X64",
        })
        self.assertEqual(manifest["result"]["selected_candidate"], "region:send")
        self.assertNotIn("capture_ids", json.dumps(manifest))
        self.assertNotIn("coordinates", json.dumps(manifest))

    def test_rejects_unverified_or_unhealthy_installed_state(self):
        inputs = self.inputs()
        status = json.loads(inputs["extension_status"].read_text())
        status["trust"] = "developer-unsigned-local"
        inputs["extension_status"].write_text(json.dumps(status))
        with self.assertRaisesRegex(ValueError, "review-only publisher-verified"):
            sanitizer.build_manifest(**inputs)

        inputs = self.inputs()
        status = json.loads(inputs["extension_status"].read_text())
        status["healthy"] = False
        inputs["extension_status"].write_text(json.dumps(status))
        with self.assertRaisesRegex(ValueError, "review-only publisher-verified"):
            sanitizer.build_manifest(**inputs)

    def test_rejects_malformed_chooser_result(self):
        inputs = self.inputs()
        choice = json.loads(inputs["chooser_result"].read_text())
        choice["tool_arguments"] = {"x": 394, "y": 270}
        inputs["chooser_result"].write_text(json.dumps(choice))
        with self.assertRaisesRegex(ValueError, "cua.jev_choice_v1"):
            sanitizer.build_manifest(**inputs)

        inputs = self.inputs()
        choice = json.loads(inputs["chooser_result"].read_text())
        choice["probabilities"] = {"region:send": 0.4}
        inputs["chooser_result"].write_text(json.dumps(choice))
        manifest = sanitizer.build_manifest(**inputs)
        self.assertEqual(manifest["result"]["selected_candidate"], "region:send")

        inputs = self.inputs()
        choice = json.loads(inputs["chooser_result"].read_text())
        choice["probabilities"] = {"region:send": 1.1}
        inputs["chooser_result"].write_text(json.dumps(choice))
        with self.assertRaisesRegex(ValueError, "invalid probability"):
            sanitizer.build_manifest(**inputs)

    def test_nullable_model_and_live_provider_are_preserved(self):
        inputs = self.inputs()
        choice = json.loads(inputs["chooser_result"].read_text())
        choice["model"] = None
        inputs["chooser_result"].write_text(json.dumps(choice))
        inputs["chooser_mode"] = "live"
        manifest = sanitizer.build_manifest(**inputs)
        self.assertEqual(manifest["runtime"]["chooser"], {
            "mode": "live",
            "provider": "typesafe",
            "model_id": None,
            "adapter_source_sha": "a" * 40,
            "source_sha": "e" * 40,
        })

    def test_raw_evidence_digest_changes_without_exposing_raw_fields(self):
        inputs = self.inputs()
        first = sanitizer.build_manifest(**inputs)
        inputs["raw_evidence"].write_text('{"capture_ids":{"acted":"different","fresh":"fresh-capture"}}')
        second = sanitizer.build_manifest(**inputs)
        self.assertNotEqual(first["raw_evidence_sha256"], second["raw_evidence_sha256"])
        self.assertNotIn("different", json.dumps(second))

    def test_source_and_platform_are_runtime_measured(self):
        repo_root = HERE.parents[3]
        measured = subprocess.run(
            ["git", "-C", repo_root, "rev-parse", "HEAD"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
        self.assertEqual(sanitizer.measure_source_sha(repo_root, measured), measured)
        with self.assertRaisesRegex(ValueError, "approved workflow candidate"):
            sanitizer.measure_source_sha(repo_root, "0" * 40)
        self.assertEqual(sanitizer.measure_platform("win32", {}), "windows")
        self.assertEqual(sanitizer.measure_platform("linux", {"DISPLAY": ":99"}), "linux-x11")
        self.assertEqual(sanitizer.measure_platform("darwin", {}), "macos")

    def test_schema_is_closed_and_requires_ed25519_trust_evidence(self):
        schema = json.loads((HERE / "evidence-manifest.schema.json").read_text())
        perception = schema["properties"]["runtime"]["properties"]["perception"]
        chooser = schema["properties"]["runtime"]["properties"]["chooser"]
        self.assertFalse(schema["additionalProperties"])
        self.assertFalse(perception["additionalProperties"])
        self.assertEqual(perception["properties"]["signature_algorithm"]["const"], "ed25519")
        self.assertEqual(perception["properties"]["trust"]["const"], "review-only-publisher-verified")
        self.assertIn("catalog_version", perception["required"])
        self.assertIn("signed_extension_archive_sha256", perception["required"])
        self.assertIn("signed_catalog_sha256", perception["required"])
        self.assertIn("driver", schema["properties"]["runtime"]["required"])
        self.assertFalse(chooser["additionalProperties"])
        self.assertEqual(chooser["properties"]["provider"]["enum"], ["fixture", "typesafe"])
        self.assertIn("null", chooser["properties"]["model_id"]["type"])
        self.assertIn("raw_evidence_sha256", schema["required"])
        self.assertIn("observation", schema["required"])
        self.assertIn("os", schema["required"])


if __name__ == "__main__":
    unittest.main()

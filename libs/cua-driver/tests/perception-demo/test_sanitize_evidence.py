import importlib.util
import base64
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock


HERE = Path(__file__).parent
SPEC = importlib.util.spec_from_file_location("sanitize_evidence", HERE / "sanitize_evidence.py")
sanitizer = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(sanitizer)


class EvidenceSanitizerTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.probe = mock.patch.object(sanitizer, "probe_recording", return_value={
            "width": 1920, "height": 1080,
            "frame_rate": {"numerator": 30, "denominator": 1},
            "duration_ms": 1250,
        })
        self.probe.start()

    def tearDown(self):
        self.probe.stop()
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
            "code_signing": {
                "status": "not-applicable",
                "format": "none",
                "identity": "none",
                "certificate_sha256": None,
                "designated_requirement": None,
            },
            "review_driver_version": "0.28.2",
            "extension_version": "0.1.0",
            "protocol_version": 1,
            "worker_sha256": "1" * 64,
            "models": [
                {"role": "icon-detect", "id": "omniparser.onnx", "revision": "a" * 40,
                 "original_sha256": "2" * 64, "converted_sha256": sanitizer.sha256_file(model), "license": "AGPL-3.0-only"},
                {"role": "ocr-detect", "id": "ppocr-det.onnx", "revision": "b" * 40,
                 "original_sha256": "3" * 64, "converted_sha256": "3" * 64, "license": "Apache-2.0"},
                {"role": "ocr-recognize", "id": "ppocr-rec.onnx", "revision": "c" * 40,
                 "original_sha256": "4" * 64, "converted_sha256": "4" * 64, "license": "Apache-2.0"},
            ],
            "onnx_runtime": {"revision": "1.26.0", "sha256": "5" * 64, "license": "MIT"},
            "self_test": {"status": "passed", "mismatch_rejection": True, "evidence_sha256": "6" * 64},
            "sealed_artifact_manifest_sha256": "7" * 64,
            "sealed_extension_manifest_sha256": "8" * 64,
        }))
        oracle.write_text(json.dumps({
            "fixture": "visual-only-canvas/v1",
            "ready": True,
            "selected": "send",
            "action_count": 1,
        }))
        recording.write_bytes(b"measured-video-bytes")
        raw_evidence.write_text(json.dumps({
            "schema": "cua-visual-perception-demo-raw/v2",
            "source_sha": "a" * 40,
            "jev_source_sha": "e" * 40,
            "platform": "linux-x11",
            "capture_ids": {"acted": "private-capture", "fresh": "fresh-capture"},
            "observation": {"input_scope": "window", "capture_kind": "get_window_state",
                            "capture_source": "driver-screenshot", "width": 760, "height": 460,
                            "desktop_session": "x11-openbox", "runner_identity_class": "github-hosted",
                            "delivery_mode": "background"},
            "chooser": {"request": {"candidates": [
                {"id": "region:send", "description": "Activate Send."},
                {"id": "reobserve", "description": "Capture again."},
                {"id": "abstain", "description": "Stop safely."},
            ]}, "mode": "mock", "response": {
                "schema": "cua.jev_choice_v1",
                "selected_id": "region:send",
                "model": "mock",
                "confidence": 1.0,
                "probabilities": {"region:send": 1.0, "reobserve": 0.0, "abstain": 0.0},
            }},
            "fixture_oracle": {
                "fixture": "visual-only-canvas/v1", "ready": True,
                "selected": "send", "action_count": 1,
            },
            "extension_status": json.loads(extension_status.read_text()),
            "parser": {"model_id": "cua-perception/demo-v1"},
            "resolved_action": {"candidate_id": "region:send", "x": 394.0, "y": 270.0},
            "verification": {
                "oracle": "passed",
                "background_desktop_refused": None,
                "capture_preserved_after_refusal": None,
                "stale_capture_refused": True,
            },
            "timeline": {"duration_ms": 1250, "events": [
                "observed", "parsed", "chosen", "background_refusal_not_applicable",
                "clicked", "oracle_verified",
                "stale_capture_refused", "reobserved",
            ]},
            "recording": {"local_path": "/private/recording.mp4",
                          "sha256": sanitizer.sha256_file(recording),
                          "metadata": {"width": 1920, "height": 1080,
                                       "frame_rate": {"numerator": 30, "denominator": 1},
                                       "duration_ms": 1250}},
        }))
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
        self.assertEqual(manifest["schema"], "cua-visual-perception-demo-evidence/v3")
        self.assertEqual(manifest["raw_evidence_sha256"], sanitizer.sha256_file(inputs["raw_evidence"]))
        self.assertEqual(perception["trust"], "review-only-publisher-verified")
        self.assertEqual(perception["signature_algorithm"], "ed25519")
        self.assertEqual(perception["publisher_key_id"], "review-only-build-override")
        self.assertEqual(perception["catalog_version"], 7)
        self.assertEqual(perception["signed_extension_archive_sha256"], "b" * 64)
        public_key = bytes(range(32))
        self.assertEqual(perception["signing_key_sha256"], sanitizer.hashlib.sha256(public_key).hexdigest())
        self.assertEqual(perception["signed_catalog_sha256"], "d" * 64)
        self.assertEqual(manifest["runtime"]["driver"]["source_sha"], "a" * 40)
        self.assertEqual(manifest["runtime"]["driver"]["version"], "0.28.2")
        self.assertEqual(manifest["runtime"]["driver"]["binary_sha256"],
                         sanitizer.sha256_file(inputs["driver_binary"]))
        self.assertEqual(manifest["runtime"]["driver"]["build"], {
            "profile": "debug-review-trust-root", "target": "x86_64-unknown-linux-gnu",
            "sealed_artifact_manifest_sha256": "7" * 64,
            "sealed_extension_manifest_sha256": "8" * 64,
        })
        self.assertEqual(manifest["runtime"]["perception"]["models"][0]["role"], "icon-detect")
        self.assertEqual(manifest["runtime"]["perception"]["onnx_runtime"]["revision"], "1.26.0")
        self.assertEqual(manifest["runtime"]["perception"]["self_test"]["status"], "passed")
        self.assertEqual({
            "source_sha": "a" * 40,
            "binary_sha256": sanitizer.sha256_file(inputs["driver_binary"]),
        }, {key: manifest["runtime"]["driver"][key] for key in ("source_sha", "binary_sha256")})
        self.assertEqual(manifest["runtime"]["chooser"]["provider"], "fixture")
        self.assertEqual(manifest["runtime"]["chooser"]["model_id"], "mock")
        self.assertEqual(manifest["runtime"]["chooser"]["adapter_source_sha"], "a" * 40)
        self.assertEqual(manifest["runtime"]["chooser"]["source_sha"], "e" * 40)
        self.assertEqual(manifest["observation"]["input_scope"], "window")
        self.assertEqual(manifest["observation"]["dimensions"], {"width": 760, "height": 460})
        self.assertEqual(len(manifest["observation"]["candidates"]), 3)
        self.assertEqual(manifest["environment"]["os"], {"name": "ubuntu22", "version": "20260915.1", "arch": "X64"})
        self.assertEqual(manifest["recording"]["original_dimensions"], {"width": 1920, "height": 1080})
        self.assertEqual(manifest["recording"]["frame_rate"], {"numerator": 30, "denominator": 1})
        self.assertEqual(manifest["result"]["selected_candidate"], "region:send")
        self.assertIsNone(manifest["result"]["background_desktop_refused"])
        self.assertIsNone(manifest["result"]["capture_preserved_after_refusal"])
        self.assertNotIn("capture_ids", json.dumps(manifest))
        self.assertNotIn("coordinates", json.dumps(manifest))

    def test_primary_desktop_observation_is_a_closed_foreground_route(self):
        inputs = self.inputs()
        raw = json.loads(inputs["raw_evidence"].read_text())
        raw["observation"].update({
            "input_scope": "desktop",
            "capture_kind": "get_desktop_state",
            "delivery_mode": "foreground",
        })
        raw["verification"].update({
            "background_desktop_refused": True,
            "capture_preserved_after_refusal": True,
        })
        raw["timeline"]["events"][3] = "background_desktop_refused"
        inputs["raw_evidence"].write_text(json.dumps(raw))
        manifest = sanitizer.build_manifest(**inputs)
        self.assertEqual(manifest["observation"]["input_scope"], "desktop")
        self.assertEqual(manifest["observation"]["capture_kind"], "get_desktop_state")
        self.assertEqual(manifest["environment"]["delivery_mode"], "foreground")
        self.assertTrue(manifest["result"]["background_desktop_refused"])
        self.assertTrue(manifest["result"]["capture_preserved_after_refusal"])

        raw["observation"]["delivery_mode"] = "background"
        inputs["raw_evidence"].write_text(json.dumps(raw))
        with self.assertRaisesRegex(ValueError, "observation context"):
            sanitizer.build_manifest(**inputs)

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
        raw = json.loads(inputs["raw_evidence"].read_text())
        raw["chooser"]["response"] = choice
        inputs["raw_evidence"].write_text(json.dumps(raw))
        manifest = sanitizer.build_manifest(**inputs)
        self.assertEqual(manifest["result"]["selected_candidate"], "region:send")

        inputs = self.inputs()
        choice = json.loads(inputs["chooser_result"].read_text())
        choice["probabilities"] = {"region:send": 1.1}
        inputs["chooser_result"].write_text(json.dumps(choice))
        with self.assertRaisesRegex(ValueError, "invalid probability"):
            sanitizer.build_manifest(**inputs)

    def test_live_provider_model_identity_is_preserved(self):
        inputs = self.inputs()
        choice = json.loads(inputs["chooser_result"].read_text())
        choice["model"] = "provider-returned-model"
        inputs["chooser_result"].write_text(json.dumps(choice))
        inputs["chooser_mode"] = "live"
        raw = json.loads(inputs["raw_evidence"].read_text())
        raw["chooser"]["mode"] = "live"
        raw["chooser"]["response"] = choice
        inputs["raw_evidence"].write_text(json.dumps(raw))
        manifest = sanitizer.build_manifest(**inputs)
        self.assertEqual(manifest["runtime"]["chooser"], {
            "mode": "live",
            "provider": "typesafe",
            "model_id": "provider-returned-model",
            "adapter_source_sha": "a" * 40,
            "source_sha": "e" * 40,
        })

    def test_live_provider_must_return_model_identity(self):
        inputs = self.inputs()
        choice = json.loads(inputs["chooser_result"].read_text())
        choice["model"] = None
        inputs["chooser_result"].write_text(json.dumps(choice))
        inputs["chooser_mode"] = "live"
        with self.assertRaisesRegex(ValueError, "return its model identity"):
            sanitizer.build_manifest(**inputs)

    def test_rejects_incomplete_sealed_model_measurements(self):
        inputs = self.inputs()
        measurements = json.loads(inputs["candidate_measurements"].read_text())
        measurements["models"].pop()
        inputs["candidate_measurements"].write_text(json.dumps(measurements))
        with self.assertRaisesRegex(ValueError, "all three ordered model identities"):
            sanitizer.build_manifest(**inputs)

    def test_requires_truthful_platform_code_signing_measurement(self):
        inputs = self.inputs()
        measurements = json.loads(inputs["candidate_measurements"].read_text())
        measurements["code_signing"]["status"] = "verified"
        inputs["candidate_measurements"].write_text(json.dumps(measurements))
        with self.assertRaisesRegex(ValueError, "misleading code-signing evidence"):
            sanitizer.build_manifest(**inputs)

        inputs = self.inputs()
        measurements = json.loads(inputs["candidate_measurements"].read_text())
        measurements["target"] = "aarch64-apple-darwin"
        measurements["code_signing"] = {
            "status": "verified",
            "format": "apple-codesign",
            "identity": "ephemeral-self-signed-review-only",
            "certificate_sha256": "9" * 64,
            "designated_requirement": 'identifier "cua-driver" and certificate leaf = H"00"',
        }
        inputs["candidate_measurements"].write_text(json.dumps(measurements))
        inputs["platform"] = "macos"
        raw = json.loads(inputs["raw_evidence"].read_text())
        raw["platform"] = "macos"
        inputs["raw_evidence"].write_text(json.dumps(raw))
        sanitizer.build_manifest(**inputs)

    def test_raw_evidence_digest_changes_without_exposing_raw_fields(self):
        inputs = self.inputs()
        first = sanitizer.build_manifest(**inputs)
        raw = json.loads(inputs["raw_evidence"].read_text())
        raw["capture_ids"]["acted"] = "different"
        inputs["raw_evidence"].write_text(json.dumps(raw))
        second = sanitizer.build_manifest(**inputs)
        self.assertNotEqual(first["raw_evidence_sha256"], second["raw_evidence_sha256"])
        self.assertNotIn("different", json.dumps(second))

    def test_rejects_unproven_stale_capture_refusal(self):
        inputs = self.inputs()
        raw = json.loads(inputs["raw_evidence"].read_text())
        raw["verification"]["stale_capture_refused"] = False
        inputs["raw_evidence"].write_text(json.dumps(raw))
        with self.assertRaisesRegex(ValueError, "stale-capture refusal"):
            sanitizer.build_manifest(**inputs)

    def test_rejects_desktop_evidence_without_background_refusal_and_capture_preservation(self):
        inputs = self.inputs()
        raw = json.loads(inputs["raw_evidence"].read_text())
        raw["observation"].update({
            "input_scope": "desktop",
            "capture_kind": "get_desktop_state",
            "delivery_mode": "foreground",
        })
        raw["timeline"]["events"][3] = "background_desktop_refused"
        inputs["raw_evidence"].write_text(json.dumps(raw))
        with self.assertRaisesRegex(ValueError, "desktop/background refusal"):
            sanitizer.build_manifest(**inputs)

        inputs = self.inputs()
        raw = json.loads(inputs["raw_evidence"].read_text())
        raw["observation"].update({
            "input_scope": "desktop",
            "capture_kind": "get_desktop_state",
            "delivery_mode": "foreground",
        })
        raw["verification"].update({
            "background_desktop_refused": True,
            "capture_preserved_after_refusal": False,
        })
        raw["timeline"]["events"][3] = "background_desktop_refused"
        inputs["raw_evidence"].write_text(json.dumps(raw))
        with self.assertRaisesRegex(ValueError, "capture preservation"):
            sanitizer.build_manifest(**inputs)

        inputs = self.inputs()
        raw = json.loads(inputs["raw_evidence"].read_text())
        raw["observation"].update({
            "input_scope": "desktop",
            "capture_kind": "get_desktop_state",
            "delivery_mode": "foreground",
        })
        raw["verification"].update({
            "background_desktop_refused": False,
            "capture_preserved_after_refusal": True,
        })
        raw["timeline"]["events"][3] = "background_desktop_refused"
        inputs["raw_evidence"].write_text(json.dumps(raw))
        with self.assertRaisesRegex(ValueError, "desktop/background refusal"):
            sanitizer.build_manifest(**inputs)

    def test_rejects_boolean_proof_for_window_route(self):
        inputs = self.inputs()
        raw = json.loads(inputs["raw_evidence"].read_text())
        raw["verification"].update({
            "background_desktop_refused": False,
            "capture_preserved_after_refusal": False,
        })
        inputs["raw_evidence"].write_text(json.dumps(raw))
        with self.assertRaisesRegex(ValueError, "desktop/background refusal"):
            sanitizer.build_manifest(**inputs)

    def test_rejects_raw_source_or_measured_result_mismatch(self):
        inputs = self.inputs()
        raw = json.loads(inputs["raw_evidence"].read_text())
        raw["source_sha"] = "f" * 40
        inputs["raw_evidence"].write_text(json.dumps(raw))
        with self.assertRaisesRegex(ValueError, "approved sources"):
            sanitizer.build_manifest(**inputs)

        inputs = self.inputs()
        raw = json.loads(inputs["raw_evidence"].read_text())
        raw["chooser"]["response"]["selected_id"] = "abstain"
        inputs["raw_evidence"].write_text(json.dumps(raw))
        with self.assertRaisesRegex(ValueError, "measured chooser result"):
            sanitizer.build_manifest(**inputs)

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
        result = schema["properties"]["result"]
        self.assertFalse(schema["additionalProperties"])
        self.assertFalse(perception["additionalProperties"])
        self.assertEqual(perception["properties"]["signature_algorithm"]["const"], "ed25519")
        self.assertEqual(perception["properties"]["trust"]["const"], "review-only-publisher-verified")
        self.assertIn("catalog_version", perception["required"])
        self.assertIn("signed_extension_archive_sha256", perception["required"])
        self.assertIn("signed_catalog_sha256", perception["required"])
        self.assertIn("driver", schema["properties"]["runtime"]["required"])
        self.assertFalse(chooser["additionalProperties"])
        self.assertFalse(result["additionalProperties"])
        self.assertEqual(chooser["properties"]["provider"]["enum"], ["fixture", "typesafe"])
        self.assertIn("null", chooser["properties"]["model_id"]["type"])
        self.assertIn("worker_sha256", perception["required"])
        self.assertIn("models", perception["required"])
        model_items = perception["properties"]["models"]["prefixItems"]
        self.assertEqual(
            [item["allOf"][1]["properties"]["role"]["const"] for item in model_items],
            ["icon-detect", "ocr-detect", "ocr-recognize"],
        )
        self.assertIn("onnx_runtime", perception["required"])
        self.assertIn("raw_evidence_sha256", schema["required"])
        self.assertIn("observation", schema["required"])
        self.assertIn("environment", schema["required"])
        self.assertIn("recording", schema["required"])
        self.assertEqual(len(schema["allOf"][0]["oneOf"]), 2)
        window_route, desktop_route = schema["allOf"][0]["oneOf"]
        self.assertIsNone(window_route["properties"]["result"]["properties"]["background_desktop_refused"]["const"])
        self.assertIsNone(window_route["properties"]["result"]["properties"]["capture_preserved_after_refusal"]["const"])
        self.assertTrue(desktop_route["properties"]["result"]["properties"]["background_desktop_refused"]["const"])
        self.assertTrue(desktop_route["properties"]["result"]["properties"]["capture_preserved_after_refusal"]["const"])


if __name__ == "__main__":
    unittest.main()

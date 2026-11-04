import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock


HERE = Path(__file__).parent
SPEC = importlib.util.spec_from_file_location("sanitize_derived_reel", HERE / "sanitize_derived_reel.py")
reel = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(reel)


class DerivedReelEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.reel_path = self.root / "derived.mp4"
        self.reel_path.write_bytes(b"derived-reel")
        self.video_probes = mock.patch.object(
            reel,
            "probe_video",
            side_effect=lambda path, decode: {
                "duration_ms": 2000 if decode else 5000,
                "frame_duration_ms": 1000 / 30,
            },
        )
        self.probe = self.video_probes.start()

    def tearDown(self):
        self.video_probes.stop()
        self.temporary.cleanup()

    def source(self, source_id="macos", platform="macos", source_sha="a" * 40):
        directory = self.root / source_id
        directory.mkdir()
        recording = directory / "recording.mp4"
        recording.write_bytes((source_id + "-recording").encode())
        digest = reel.sha256_file(recording)
        manifest = {
            "schema": "cua-visual-perception-demo-evidence/v3",
            "platform": platform,
            "fixture": {"id": "visual-only-canvas/v1"},
            "runtime": {
                "driver": {"source_sha": source_sha},
                "perception": {"trust": "review-only-publisher-verified", "signature_algorithm": "ed25519"},
                "chooser": {"mode": "live", "provider": "typesafe", "model_id": "jev-model", "source_sha": "b" * 40},
            },
            "result": {"status": "passed", "stale_capture_refused": True},
            "recording": {"final_sha256": digest, "size_bytes": recording.stat().st_size},
            "artifacts": [{"kind": "video", "path": "recording.mp4", "sha256": digest, "size_bytes": recording.stat().st_size}],
        }
        manifest_path = directory / "manifest.json"
        manifest_path.write_text(json.dumps(manifest))
        return {"id": source_id, "evidence_manifest": f"{source_id}/manifest.json", "recording": f"{source_id}/recording.mp4"}

    def plan(self):
        source = self.source()
        plan = self.root / "edit-plan.json"
        plan.write_text(json.dumps({
            "schema": "cua-derived-reel-edit-plan/v1",
            "sources": [source],
            "shots": [
                {"source_id": "macos", "start_ms": 500, "end_ms": 1500},
                {"source_id": "macos", "start_ms": 3000, "end_ms": 4000},
            ],
            "wait_cuts": [{
                "source_id": "macos", "start_ms": 1500, "end_ms": 3000,
                "disclosure": reel.WAIT_DISCLOSURE,
            }],
        }))
        return plan

    def test_manifest_binds_sources_ranges_edits_and_full_decode(self):
        manifest = reel.build_manifest(source_root=self.root, edit_plan=self.plan(), reel=self.reel_path)
        reel.validate_manifest_schema(manifest)
        self.assertEqual(manifest["schema"], "cua-visual-perception-derived-reel/v1")
        self.assertEqual(manifest["identity"]["source_sha"], "a" * 40)
        self.assertEqual(manifest["identity"]["jev_source_sha"], "b" * 40)
        source = manifest["sources"][0]
        self.assertEqual(source["used_ranges"], [
            {"start_ms": 500, "end_ms": 1500},
            {"start_ms": 3000, "end_ms": 4000},
        ])
        self.assertEqual(source["removed_wait_ranges"], [{
            "start_ms": 1500, "end_ms": 3000, "disclosure": reel.WAIT_DISCLOSURE,
        }])
        self.assertEqual([item["operation"] for item in manifest["edit_operations"]], ["trim", "trim", "concatenate"])
        self.assertEqual(manifest["recording"]["expected_duration_ms"], 2000)
        self.assertTrue(manifest["recording"]["fully_decoded"])
        self.assertEqual(manifest["artifacts"][0]["sha256"], reel.sha256_file(self.reel_path))
        self.assertNotIn(str(self.root), json.dumps(manifest))
        self.assertEqual(self.probe.call_args_list[-1].kwargs, {"decode": True})

    def test_rejects_undisclosed_or_misrepresented_wait_cut(self):
        plan_path = self.plan()
        value = json.loads(plan_path.read_text())
        value["wait_cuts"] = []
        plan_path.write_text(json.dumps(value))
        with self.assertRaisesRegex(ValueError, "every and only"):
            reel.build_manifest(source_root=self.root, edit_plan=plan_path, reel=self.reel_path)

        value["wait_cuts"] = [{
            "source_id": "macos", "start_ms": 1500, "end_ms": 3000,
            "disclosure": "probably idle",
        }]
        plan_path.write_text(json.dumps(value))
        with self.assertRaisesRegex(ValueError, "exact no-action-omitted"):
            reel.build_manifest(source_root=self.root, edit_plan=plan_path, reel=self.reel_path)

    def test_rejects_source_hash_or_identity_mismatch(self):
        plan_path = self.plan()
        (self.root / "macos" / "recording.mp4").write_bytes(b"changed")
        with self.assertRaisesRegex(ValueError, "differ from the source evidence"):
            reel.build_manifest(source_root=self.root, edit_plan=plan_path, reel=self.reel_path)

    def test_rejects_reel_duration_that_does_not_match_edits(self):
        plan_path = self.plan()
        self.video_probes.stop()
        with mock.patch.object(reel, "probe_video", side_effect=lambda path, decode: {
            "duration_ms": 2600 if decode else 5000, "frame_duration_ms": 1000 / 30,
        }):
            with self.assertRaisesRegex(ValueError, "duration does not match"):
                reel.build_manifest(source_root=self.root, edit_plan=plan_path, reel=self.reel_path)
        self.probe = self.video_probes.start()

    def test_rejects_path_escape_and_too_few_real_edits(self):
        plan_path = self.plan()
        value = json.loads(plan_path.read_text())
        value["sources"][0]["recording"] = "../private.mp4"
        plan_path.write_text(json.dumps(value))
        with self.assertRaisesRegex(ValueError, "safe relative"):
            reel.build_manifest(source_root=self.root, edit_plan=plan_path, reel=self.reel_path)

        value["sources"][0]["recording"] = "macos/recording.mp4"
        value["shots"] = value["shots"][:1]
        value["wait_cuts"] = []
        plan_path.write_text(json.dumps(value))
        with self.assertRaisesRegex(ValueError, "at least two"):
            reel.build_manifest(source_root=self.root, edit_plan=plan_path, reel=self.reel_path)


if __name__ == "__main__":
    unittest.main()

import importlib.util
import json
from pathlib import Path
import tempfile
import unittest


HERE = Path(__file__).parent
SPEC = importlib.util.spec_from_file_location("sanitize_evidence", HERE / "sanitize_evidence.py")
sanitizer = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(sanitizer)


def valid_input():
    return {
        "source_sha": "a" * 40,
        "platform": "linux-x11",
        "fixture": {"id": "visual-only-canvas/v1", "oracle": {"selected": "tide", "action_count": 1}},
        "runtime": {"adapter": "jev-use", "model_id": "jev/demo-v1", "signed_extension_sha256": "b" * 64},
        "result": {"status": "passed"},
    }


class EvidenceSanitizerTests(unittest.TestCase):
    def test_emits_only_review_safe_manifest_fields(self):
        with tempfile.TemporaryDirectory() as directory:
            video = Path(directory) / "raw.mp4"
            video.write_bytes(b"synthetic-video")
            manifest = sanitizer.sanitize(valid_input(), video)
        self.assertEqual(set(manifest), {"schema", "source_sha", "platform", "fixture", "runtime", "result", "artifacts"})
        self.assertEqual(manifest["artifacts"][0]["path"], "recording.mp4")
        self.assertNotIn("api_key", json.dumps(manifest).lower())

    def test_rejects_unknown_or_secret_bearing_fields(self):
        raw = valid_input()
        raw["logs"] = "TYPESAFE_API_KEY=secret"
        with tempfile.NamedTemporaryFile() as video:
            with self.assertRaisesRegex(ValueError, "input keys"):
                sanitizer.sanitize(raw, Path(video.name))

    def test_schema_forbids_extra_properties_and_non_mp4_artifacts(self):
        schema = json.loads((HERE / "evidence-manifest.schema.json").read_text())
        self.assertFalse(schema["additionalProperties"])
        self.assertEqual(schema["properties"]["artifacts"]["maxItems"], 1)
        self.assertEqual(schema["properties"]["artifacts"]["items"]["properties"]["path"]["const"], "recording.mp4")


if __name__ == "__main__":
    unittest.main()

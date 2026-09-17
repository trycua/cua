import hashlib
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


HERE = Path(__file__).parent
SPEC = importlib.util.spec_from_file_location("sanitize_evidence", HERE / "sanitize_evidence.py")
sanitizer = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(sanitizer)


class EvidenceSanitizerTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if shutil.which("openssl") is None:
            raise unittest.SkipTest("openssl is required for signature verification tests")
        cls.temporary = tempfile.TemporaryDirectory()
        cls.root = Path(cls.temporary.name)
        cls.private_key = cls.root / "private.pem"
        cls.public_key = cls.root / "public.pem"
        subprocess.run(
            [
                "openssl",
                "genpkey",
                "-algorithm",
                "RSA",
                "-pkeyopt",
                "rsa_keygen_bits:2048",
                "-out",
                cls.private_key,
            ],
            check=True,
            capture_output=True,
        )
        subprocess.run(
            ["openssl", "pkey", "-in", cls.private_key, "-pubout", "-out", cls.public_key],
            check=True,
            capture_output=True,
        )

    @classmethod
    def tearDownClass(cls):
        cls.temporary.cleanup()

    def inputs(self):
        extension = self.root / "extension.bin"
        signature = self.root / "extension.sig"
        model = self.root / "model.bin"
        adapter_result = self.root / "adapter-result.json"
        oracle = self.root / "oracle.json"
        recording = self.root / "recording.mp4"
        extension.write_bytes(b"signed-extension-bytes")
        model.write_bytes(b"measured-model-bytes")
        adapter_result.write_text(
            json.dumps(
                {
                    "adapter": "jev-use",
                    "status": "passed",
                    "source_sha": "a" * 40,
                    "model_id": "jev/demo-v1",
                }
            )
        )
        recording.write_bytes(b"measured-video-bytes")
        oracle.write_text(
            json.dumps(
                {
                    "fixture": "visual-only-canvas/v1",
                    "ready": True,
                    "selected": "tide",
                    "action_count": 2,
                }
            )
        )
        subprocess.run(
            [
                "openssl",
                "dgst",
                "-sha256",
                "-sign",
                self.private_key,
                "-out",
                signature,
                extension,
            ],
            check=True,
            capture_output=True,
        )
        return {
            "source_sha": "a" * 40,
            "platform": "linux-x11",
            "extension": extension,
            "extension_signature": signature,
            "trusted_public_key": self.public_key,
            "trusted_public_key_sha256": hashlib.sha256(self.public_key.read_bytes()).hexdigest(),
            "model": model,
            "adapter_result": adapter_result,
            "oracle": oracle,
            "recording": recording,
        }

    def test_manifest_is_bound_to_measured_runtime_inputs(self):
        inputs = self.inputs()
        manifest = sanitizer.build_manifest(**inputs)
        self.assertEqual(manifest["source_sha"], inputs["source_sha"])
        self.assertEqual(manifest["platform"], inputs["platform"])
        self.assertEqual(manifest["runtime"]["model_id"], "jev/demo-v1")
        self.assertEqual(manifest["result"]["status"], "passed")
        self.assertEqual(manifest["fixture"]["oracle"], {"selected": "tide", "action_count": 2})
        self.assertEqual(
            manifest["runtime"]["model_sha256"], sanitizer.sha256_file(inputs["model"])
        )
        self.assertEqual(
            manifest["runtime"]["signed_extension_sha256"],
            sanitizer.sha256_file(inputs["extension"]),
        )
        self.assertEqual(manifest["runtime"]["signature_algorithm"], "rsa-sha256")
        self.assertEqual(
            manifest["artifacts"][0]["sha256"],
            sanitizer.sha256_file(inputs["recording"]),
        )

    def test_rejects_tampered_extension_bytes(self):
        inputs = self.inputs()
        inputs["extension"].write_bytes(b"tampered-after-signing")
        with self.assertRaisesRegex(ValueError, "failed RSA-SHA256 verification"):
            sanitizer.build_manifest(**inputs)

    def test_rejects_unapproved_signing_key_bytes(self):
        inputs = self.inputs()
        inputs["trusted_public_key_sha256"] = "0" * 64
        with self.assertRaisesRegex(ValueError, "approved digest"):
            sanitizer.build_manifest(**inputs)

    def test_rejects_adapter_result_for_another_source(self):
        inputs = self.inputs()
        result = json.loads(inputs["adapter_result"].read_text())
        result["source_sha"] = "b" * 40
        inputs["adapter_result"].write_text(json.dumps(result))
        with self.assertRaisesRegex(ValueError, "measured checkout"):
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
        with self.assertRaisesRegex(ValueError, "active Linux X11"):
            sanitizer.measure_platform("linux", {})

    def test_schema_forbids_extra_properties_and_records_measured_digests(self):
        schema = json.loads((HERE / "evidence-manifest.schema.json").read_text())
        runtime = schema["properties"]["runtime"]
        self.assertFalse(schema["additionalProperties"])
        self.assertFalse(runtime["additionalProperties"])
        self.assertIn("model_sha256", runtime["required"])
        self.assertIn("signing_key_sha256", runtime["required"])
        artifacts = schema["properties"]["artifacts"]
        self.assertEqual(artifacts["maxItems"], 1)
        self.assertEqual(artifacts["items"]["properties"]["path"]["const"], "recording.mp4")


if __name__ == "__main__":
    unittest.main()

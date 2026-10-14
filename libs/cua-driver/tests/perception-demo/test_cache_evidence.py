"""Offline exact-identity and private encrypted-cache regression tests."""

import hashlib
import importlib.util
from io import BytesIO
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock
import zipfile


HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
SPEC = importlib.util.spec_from_file_location("cache_evidence", HERE / "cache_evidence.py")
cache = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(cache)


class EvidenceCacheTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.cache = self.root / "cache"
        self.run_id = 42
        self.artifact_id = 91
        self.sha = "a" * 40
        self.workflow = ".github/workflows/authorized-live-jev-use-demo.yml"
        self.name = "encrypted-evidence"
        output = BytesIO()
        with zipfile.ZipFile(output, "w") as archive:
            for name in cache.MEMBERS:
                archive.writestr(name, b"CUAEVID\x00" + name.encode())
        self.data = output.getvalue()
        self.expected = {
            "schema": "cua-encrypted-evidence-cache/v1", "repository": cache.REPO,
            "run_id": self.run_id, "artifact_id": self.artifact_id,
            "source_sha": self.sha, "workflow": self.workflow, "name": self.name,
            "archive_digest": "sha256:" + hashlib.sha256(self.data).hexdigest(),
        }

    def remote(self, *, run=None, artifact=None):
        run = run or {
            "id": self.run_id, "head_sha": self.sha, "path": self.workflow,
            "repository": {"full_name": cache.REPO, "id": 8},
            "status": "completed", "conclusion": "success",
        }
        artifact = artifact or {
            "id": self.artifact_id, "name": self.name, "expired": False,
            "workflow_run": {"id": self.run_id, "head_sha": self.sha, "repository_id": 8},
            "digest": self.expected["archive_digest"],
        }
        return mock.patch.object(cache, "gh_json", side_effect=[run, artifact])

    def test_identity_checks_run_and_immutable_artifact(self):
        with self.remote():
            self.assertEqual(cache.identity(self.run_id, self.artifact_id, self.sha, self.workflow, self.name), self.expected)
        for change, label in (
            ({"head_sha": "b" * 40}, "source"),
            ({"status": "in_progress"}, "completed"),
            ({"path": ".github/workflows/other.yml"}, "workflow"),
            ({"repository": {"full_name": "someone/else", "id": 8}}, "repository"),
        ):
            with self.subTest(change=change), self.remote(run={
                "id": self.run_id, "head_sha": self.sha, "path": self.workflow,
                "repository": {"full_name": cache.REPO, "id": 8},
                "status": "completed", "conclusion": "success", **change,
            }):
                with self.assertRaisesRegex(ValueError, label):
                    cache.identity(self.run_id, self.artifact_id, self.sha, self.workflow, self.name)
        for change, label in (
            ({"digest": None}, "digest"),
            ({"expired": True}, "expired"),
            ({"workflow_run": {"id": 43, "head_sha": self.sha, "repository_id": 8}}, "producer"),
        ):
            with self.subTest(change=change), self.remote(artifact={
                "id": self.artifact_id, "name": self.name, "expired": False,
                "workflow_run": {"id": self.run_id, "head_sha": self.sha, "repository_id": 8},
                "digest": self.expected["archive_digest"], **change,
            }):
                with self.assertRaisesRegex(ValueError, label):
                    cache.identity(self.run_id, self.artifact_id, self.sha, self.workflow, self.name)

    def write_cached(self):
        self.cache.mkdir(mode=0o700)
        archive, metadata = cache.paths(self.cache, self.artifact_id)
        archive.write_bytes(self.data)
        metadata.write_text(json.dumps(self.expected), encoding="utf-8")
        archive.chmod(0o600)
        metadata.chmod(0o600)
        return archive, metadata

    def test_validated_cache_can_be_staged_without_retransfer_or_plaintext(self):
        archive, _ = self.write_cached()
        with mock.patch.object(cache, "subprocess") as process:
            self.assertEqual(cache.fetch(self.cache, self.expected), archive)
            process.assert_not_called()
        destination = self.root / "stage"
        cache.stage(cache.read_cached(self.cache, self.expected), destination, self.expected["archive_digest"])
        self.assertEqual(sorted(item.name for item in destination.iterdir()), list(cache.MEMBERS))
        self.assertEqual((destination / "window.cuae").read_bytes(), b"CUAEVID\x00window.cuae")
        with self.assertRaisesRegex(ValueError, "already exists"):
            cache.stage(archive, destination, self.expected["archive_digest"])

    def test_recovers_complete_archive_after_interrupted_metadata_write(self):
        archive, metadata = self.write_cached()
        metadata.unlink()
        with mock.patch.object(cache, "subprocess") as process:
            self.assertEqual(cache.fetch(self.cache, self.expected), archive)
            process.assert_not_called()
        self.assertEqual(json.loads(metadata.read_text(encoding="utf-8")), self.expected)

    def test_fetch_streams_verified_archive_once_then_reuses_it(self):
        self.cache.mkdir(mode=0o700)

        class Download:
            def __init__(self, payload):
                self.stdout = BytesIO(payload)

            def __enter__(self):
                return self

            def __exit__(self, *_):
                return False

            def wait(self):
                return 0

            def kill(self):
                raise AssertionError("valid download must not be terminated")

        with mock.patch.object(cache.subprocess, "Popen", return_value=Download(self.data)) as download:
            archive = cache.fetch(self.cache, self.expected)
            self.assertEqual(archive.read_bytes(), self.data)
            self.assertEqual(cache.fetch(self.cache, self.expected), archive)
            download.assert_called_once()
        self.assertEqual(json.loads(cache.paths(self.cache, self.artifact_id)[1].read_text()), self.expected)

    def test_failed_download_never_publishes_an_archive(self):
        self.cache.mkdir(mode=0o700)

        class Download:
            stdout = BytesIO(b"partial")

            def __enter__(self):
                return self

            def __exit__(self, *_):
                return False

            def wait(self):
                return 1

            def kill(self):
                raise AssertionError("completed failed download is not killed")

        with mock.patch.object(cache.subprocess, "Popen", return_value=Download()):
            with self.assertRaisesRegex(ValueError, "download failed"):
                cache.fetch(self.cache, self.expected)
        self.assertFalse(any(self.cache.iterdir()))

    def test_interrupted_archive_with_wrong_digest_cannot_be_recovered(self):
        archive, metadata = self.write_cached()
        metadata.unlink()
        archive.write_bytes(b"incomplete")
        with self.assertRaisesRegex(ValueError, "digest mismatch"):
            cache.fetch(self.cache, self.expected)
        self.assertFalse(metadata.exists())

    def test_tampering_and_foreign_identity_cannot_be_reused(self):
        archive, metadata = self.write_cached()
        with self.assertRaisesRegex(ValueError, "identity changed"):
            cache.read_cached(self.cache, {**self.expected, "source_sha": "b" * 40})
        archive.write_bytes(self.data + b"altered")
        with self.assertRaisesRegex(ValueError, "digest mismatch"):
            cache.read_cached(self.cache, self.expected)
        archive.write_bytes(self.data)
        archive.chmod(0o644)
        with self.assertRaisesRegex(ValueError, "group or others"):
            cache.read_cached(self.cache, self.expected)
        archive.chmod(0o600)
        metadata.unlink()
        metadata.symlink_to(archive)
        with self.assertRaisesRegex(ValueError, "regular file"):
            cache.read_cached(self.cache, self.expected)

    def test_archive_refuses_unencrypted_extra_and_traversal_entries(self):
        archive = self.root / "evidence.zip"
        for names in (
            ("window.cuae", "primary-desktop.cuae", "manifest.json"),
            ("window.cuae", "../primary-desktop.cuae"),
        ):
            with self.subTest(names=names):
                with zipfile.ZipFile(archive, "w") as output:
                    for name in names:
                        output.writestr(name, b"CUAEVID\x00sample")
                archive.chmod(0o600)
                with self.assertRaisesRegex(ValueError, "only encrypted"):
                    cache.inspect_archive(archive)
        with zipfile.ZipFile(archive, "w") as output:
            for name in cache.MEMBERS:
                output.writestr(name, b"not an envelope")
        with self.assertRaisesRegex(ValueError, "not an evidence envelope"):
            cache.inspect_archive(archive)

    def test_private_directory_must_not_be_a_link_or_group_accessible(self):
        self.cache.mkdir(mode=0o755)
        with self.assertRaisesRegex(ValueError, "group or others"):
            cache.private_directory(self.cache)
        self.cache.chmod(0o700)
        link = self.root / "alias"
        link.symlink_to(self.cache, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "real directory"):
            cache.private_directory(link)


if __name__ == "__main__":
    unittest.main()

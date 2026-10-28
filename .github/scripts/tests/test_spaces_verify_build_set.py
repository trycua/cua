"""Fixtures for spaces_verify_build_set.py (CUA-2067)."""

import hashlib
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from spaces_verify_build_set import check_manifest  # noqa: E402

SHA = "a" * 40
V = "0.14.0"


def make_set(root: Path, **over):
    files = {"a.dmg": b"dmg", "latest.yml": b"feed"}
    entries = [
        {"file": n, "sha256": hashlib.sha256(b).hexdigest(), "version": V, "source_sha": SHA}
        for n, b in files.items()
    ]
    for n, b in files.items():
        (root / n).write_bytes(b)
    m = {"schema": 1, "version": V, "source_sha": SHA, "files": entries}
    m.update(over)
    (root / "build-manifest.json").write_text(json.dumps(m))


class VerifyTests(unittest.TestCase):
    def check(self, tag_version=V, tag_sha=SHA, mutate=None, **over):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            make_set(root, **over)
            if mutate:
                mutate(root)
            return check_manifest(root, tag_version, tag_sha)

    def test_good_set_returns_the_manifest_digest(self):
        problems, digest = self.check()
        self.assertEqual(problems, [])
        self.assertEqual(len(digest), 64)

    def test_other_commit_fails(self):
        problems, _ = self.check(tag_sha="b" * 40)
        self.assertTrue(any("not the tag commit" in p for p in problems))

    def test_other_version_fails(self):
        problems, _ = self.check(tag_version="0.14.1")
        self.assertTrue(any("not the tag version" in p for p in problems))

    def test_changed_bytes_fail(self):
        problems, _ = self.check(mutate=lambda r: (r / "a.dmg").write_bytes(b"evil"))
        self.assertTrue(any("sha256 differs" in p for p in problems))

    def test_extra_file_fails(self):
        problems, _ = self.check(mutate=lambda r: (r / "x.exe").write_bytes(b"x"))
        self.assertTrue(any("not in the manifest" in p for p in problems))

    def test_missing_file_fails(self):
        problems, _ = self.check(mutate=lambda r: (r / "a.dmg").unlink())
        self.assertTrue(any("listed but missing" in p for p in problems))

    def test_missing_manifest_fails(self):
        problems, _ = self.check(mutate=lambda r: (r / "build-manifest.json").unlink())
        self.assertEqual(problems, ["build-manifest.json is missing"])


if __name__ == "__main__":
    unittest.main()

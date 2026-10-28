"""Store tests: the fenced snapshot store and its backends (python3 -m unittest).

The S3 backend runs against a real bucket only when CUA_TEST_S3_URL is set
(e.g. s3://bucket/prefix, with AWS_* credentials and AWS_REGION in the env);
everything else is hermetic.
"""
import datetime as dt
import json
import os
import subprocess
import sys
import tempfile
import threading
import time
import unittest
import uuid
from pathlib import Path

RUNTIME = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(RUNTIME))
import cua  # noqa: E402
from cua import _store  # noqa: E402


def write(d: Path, name: str, data: bytes) -> Path:
    p = d / name
    p.write_bytes(data)
    return p


class StoreContract:
    """Store semantics every backend must keep; subclasses provide self.url."""

    url = ""

    def store(self, attempt, run="run-1"):
        return _store.CheckpointStore(self.url, run, attempt)

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.src = Path(self.tmp.name)

    def test_save_commit_latest_manifest(self):
        s = self.store(1)
        f = write(self.src, "adapters.safetensors", b"A" * 1000)
        path = s.save(10, {"adapters.safetensors": f}, world_size=2)
        self.assertEqual(path, "attempt-1/step-10")
        self.assertEqual(s.latest(), {"step": 10, "attempt": 1, "path": "attempt-1/step-10"})
        man = json.loads(s.b.get(s.k(path, "manifest.json"))[0])
        self.assertEqual((man["step"], man["attempt"], man["world_size"]), (10, 1, 2))
        self.assertEqual(man["files"]["adapters.safetensors"]["sha256"], _store.sha256_file(f))
        marker = json.loads(s.b.get(s.k("COMMITTED", "step-10"))[0])
        self.assertEqual(marker["path"], path)

    def test_latest_only_moves_forward(self):
        s = self.store(1)
        f = write(self.src, "x", b"x")
        s.save(20, {"x": f})
        self.assertIsNone(s.save(10, {"x": f}))  # older step: nothing to do
        self.assertIsNone(s.save(20, {"x": f}))  # same step: idempotent
        self.assertEqual(s.latest()["step"], 20)

    def test_marker_is_create_only(self):
        s = self.store(1)
        s.b.put(s.k("COMMITTED", "probe"), b"1", if_none_match=True)
        with self.assertRaises(_store._Conflict):
            s.b.put(s.k("COMMITTED", "probe"), b"2", if_none_match=True)
        self.assertEqual(s.b.get(s.k("COMMITTED", "probe"))[0], b"1")

    def test_scenario_39_stale_attempt_commit_rejected(self):
        """A stale attempt writes step N after the new attempt committed it: rejected."""
        old, new = self.store(1), self.store(2)
        f_old = write(self.src, "old", b"old weights")
        f_new = write(self.src, "new", b"new weights")
        old.save(50, {"w": f_old})
        # attempt 1 is cut off but still running; it uploads step 100 ...
        stale_path = old.upload(100, {"w": f_old})
        # ... meanwhile attempt 2 starts, claims the run and commits step 100.
        new.claim()
        new.save(100, {"w": f_new})
        with self.assertRaises(_store.StaleAttempt):
            old.commit(100, stale_path)
        with self.assertRaises(_store.StaleAttempt):  # a newer step is fenced too
            old.save(150, {"w": f_old})
        doc = new.latest()
        self.assertEqual(doc, {"step": 100, "attempt": 2, "path": "attempt-2/step-100"})
        out = Path(self.tmp.name) / "dl"
        new.download(doc, out)
        self.assertEqual((out / "w").read_bytes(), b"new weights")

    def test_claim_fences_before_first_commit(self):
        """Scenario 10's store half: once a new attempt starts, the cut-off one commits nothing."""
        old, new = self.store(3), self.store(4)
        f = write(self.src, "w", b"w")
        old.save(10, {"w": f})
        new.claim()
        self.assertEqual(new.latest(), {"step": 10, "attempt": 4, "path": "attempt-3/step-10"})
        with self.assertRaises(_store.StaleAttempt):
            old.save(20, {"w": f})
        self.assertEqual(new.latest()["step"], 10)

    def test_gc_keeps_last_three_and_drops_stale_uploads(self):
        s1 = self.store(1)
        f = write(self.src, "w", b"w")
        for step in (10, 20, 30, 40):
            s1.save(step, {"w": f})
        orphan = s1.upload(50, {"w": f})  # attempt 1 dies before committing 50
        s2 = self.store(2)
        s2.claim()
        s2.save(60, {"w": f})
        keys = s2.b.list("run-1/")
        dirs = sorted({"/".join(k.split("/")[1:3]) for k in keys if "/attempt-" in "/" + k})
        self.assertEqual(dirs, ["attempt-1/step-30", "attempt-1/step-40", "attempt-2/step-60"])
        self.assertNotIn(orphan, dirs)
        markers = sorted(k.split("/")[-1] for k in keys if "/COMMITTED/" in k)
        self.assertEqual(markers, ["step-10", "step-20", "step-30", "step-40", "step-60"])

    def test_download_verifies_sha256(self):
        s = self.store(1)
        f = write(self.src, "w", b"good")
        path = s.save(5, {"w": f})
        s.b.put(s.k(path, "files", "w"), b"evil")
        with self.assertRaises(_store.StoreError):
            s.download(s.latest(), Path(self.tmp.name) / "dl")
        self.assertFalse((Path(self.tmp.name) / "dl").exists())

    def test_concurrent_attempts_never_move_latest_backwards(self):
        f = write(self.src, "w", b"w")
        seen, errors = [], []
        stop = threading.Event()

        def watch():
            s = self.store(1)
            while not stop.is_set():
                doc = s.latest()
                if doc:
                    seen.append((doc["attempt"], doc["step"]))
                time.sleep(0.002)

        def worker(attempt):
            s = self.store(attempt)
            try:
                for step in range(1, 6):
                    s.save(step * 10, {"w": f})
            except _store.StaleAttempt:
                pass
            except Exception as e:  # noqa: BLE001
                errors.append(e)

        w = threading.Thread(target=watch)
        w.start()
        ts = [threading.Thread(target=worker, args=(a,)) for a in (1, 2, 3)]
        for t in ts:
            t.start()
        for t in ts:
            t.join(120)
        stop.set()
        w.join()
        self.assertEqual(errors, [])
        for a, b in zip(seen, seen[1:]):
            self.assertLessEqual(a[0], b[0], seen)
            self.assertLessEqual(a[1], b[1], seen)
        final = self.store(1).latest()
        self.assertEqual(final["attempt"], 3)


class FileStore(StoreContract, unittest.TestCase):
    def setUp(self):
        super().setUp()
        self.url = "file://" + str(Path(self.tmp.name) / "store")


@unittest.skipUnless(os.environ.get("CUA_TEST_S3_URL"), "set CUA_TEST_S3_URL=s3://bucket/prefix to test S3")
class S3Store(StoreContract, unittest.TestCase):
    def setUp(self):
        super().setUp()
        base = os.environ["CUA_TEST_S3_URL"].rstrip("/")
        self.url = f"{base}/t-{uuid.uuid4().hex[:10]}"
        self.addCleanup(self._wipe)

    def _wipe(self):
        b = _store.open_backend(self.url)
        for k in b.list(""):
            b.delete(k)

    def test_cas_if_match_on_s3(self):
        b = _store.open_backend(self.url)
        b.put_cas_new("k", b"1")
        etag = b.get("k")[1]
        b.put("k", b"2", if_match=etag)
        with self.assertRaises(_store._Conflict):
            b.put("k", b"3", if_match=etag)  # stale etag
        with self.assertRaises(_store._Conflict):
            b.put_cas_new("k", b"4")
        self.assertEqual(b.get("k")[0], b"2")


class SigV4(unittest.TestCase):
    def test_aws_documented_get_object_example(self):
        """AWS's published SigV4 example (S3 GET Object with a Range header)."""
        env = {"AWS_ACCESS_KEY_ID": "AKIAIOSFODNN7EXAMPLE",
               "AWS_SECRET_ACCESS_KEY": "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
               "AWS_REGION": "us-east-1", "AWS_ENDPOINT_URL_S3": "https://examplebucket.s3.amazonaws.com"}
        b = _store.S3Backend("examplebucket", "", env)
        empty = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        h = b._sign("GET", "/test.txt", {}, {"range": "bytes=0-9"}, empty,
                    now=dt.datetime(2013, 5, 24, tzinfo=dt.timezone.utc))
        self.assertTrue(h["authorization"].endswith(
            "Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"), h["authorization"])

    def test_url_forms(self):
        with self.assertRaises(ValueError):
            cua.check_store_url("http://x/y")
        with self.assertRaises(ValueError):
            cua.check_store_url("file://host/path")
        self.assertEqual(cua.check_store_url("s3://b/p"), "s3://b/p")
        self.assertEqual(cua.check_store_url(None), "")


class Dedup(unittest.TestCase):
    def test_unchanged_files_are_referenced_and_survive_gc(self):
        with tempfile.TemporaryDirectory() as d:
            src = Path(d)
            s = _store.CheckpointStore("file://" + str(src / "store"), "r", 1, keep=2)
            big = write(src, "big", b"B" * 1000)
            p1 = s.save(1, {"base/model.bin": big})
            meta = s.manifest(p1)["files"]["base/model.bin"]
            for step in range(2, 6):
                small = write(src, "small", f"s{step}".encode())
                s.save(step, {"adapter.bin": small}, reuse={"base/model.bin": meta})
            m5 = s.manifest(s.latest()["path"])
            self.assertEqual(m5["files"]["base/model.bin"]["src"], p1)
            keys = s.b.list("r/")
            self.assertIn("r/attempt-1/step-1/files/base/model.bin", keys)   # referenced: kept
            self.assertNotIn("r/attempt-1/step-1/manifest.json", keys)      # snapshot 1 itself: gone
            self.assertNotIn("r/attempt-1/step-2/files/adapter.bin", keys)
            out = src / "dl"
            s.download(s.latest(), out)
            self.assertEqual((out / "base" / "model.bin").read_bytes(), b"B" * 1000)
            self.assertEqual((out / "adapter.bin").read_bytes(), b"s5")

    def test_bad_paths(self):
        for bad in ("", "/abs", "a/../b", "a//b", "./a"):
            with self.assertRaises(ValueError):
                _store.check_rel(bad)


if __name__ == "__main__":
    unittest.main()

"""data_dir persistence, liveness and the start barrier (python3 -m unittest).

The S3 variant runs only when CUA_TEST_S3_URL is set (s3://bucket/prefix with
AWS_* credentials and AWS_REGION in the env); everything else is hermetic.
"""
import json
import os
import signal
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
import _cua_entry  # noqa: E402
import cua  # noqa: E402
from cua import _store, progress  # noqa: E402


def old(p: Path, secs: float = 10) -> Path:
    """Backdate p so a sync treats it as settled."""
    t = time.time() - secs
    os.utime(p, (t, t))
    return p


class DataDirSyncFile(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.url = "file://" + str(self.root / "store")

    def sync(self, host, attempt, rank=0, world=1):
        st = _store.CheckpointStore(self.url, "run", attempt)
        return _store.DataDirSync(st, self.root / host, rank, world)

    def test_first_attempt_starts_empty_and_claims(self):
        a = self.sync("a", 1)
        (self.root / "a").mkdir()
        (self.root / "a" / "leftover").write_text("from a crashed attempt")
        self.assertEqual(a.restore(), {"step": 0, "attempt": 1, "path": None})
        self.assertFalse((self.root / "a" / "leftover").exists())
        self.assertIsNone(a.sync())  # nothing to snapshot

    def test_snapshot_then_restore_on_a_new_host(self):
        a = self.sync("a", 1)
        a.restore()
        d = self.root / "a"
        (d / "w.bin").write_bytes(b"w1")
        old(d / "w.bin")
        (d / "adapters").mkdir()
        p = d / "adapters" / "0000050_adapters.safetensors"
        p.write_bytes(b"A50")
        old(p)
        (d / ".cua").mkdir()
        (d / ".cua" / "internal").write_text("never synced")
        self.assertEqual(a.sync(), "attempt-1/step-1")
        self.assertIsNone(a.sync())  # unchanged: no new snapshot
        b = self.sync("b", 2)
        doc = b.restore()
        self.assertEqual((doc["step"], doc["attempt"]), (1, 2))
        self.assertEqual((self.root / "b" / "w.bin").read_bytes(), b"w1")
        self.assertEqual((self.root / "b" / "adapters" / "0000050_adapters.safetensors").read_bytes(), b"A50")
        self.assertFalse((self.root / "b" / ".cua" / "internal").exists())

    def test_unchanged_files_are_not_uploaded_again(self):
        a = self.sync("a", 1)
        a.restore()
        d = self.root / "a"
        (d / "base.bin").write_bytes(b"B" * 100)
        old(d / "base.bin")
        a.sync()
        (d / "step.txt").write_text("2")
        old(d / "step.txt")
        path = a.sync()
        files = a.store.manifest(path)["files"]
        self.assertEqual(files["base.bin"]["src"], "attempt-1/step-1")
        self.assertEqual(files["step.txt"]["src"], "attempt-1/step-2")
        (d / "step.txt").unlink()
        path = a.sync()
        self.assertEqual(sorted(a.store.manifest(path)["files"]), ["base.bin"])

    def test_recent_files_wait_unless_final(self):
        a = self.sync("a", 1)
        a.restore()
        (self.root / "a" / "fresh").write_text("being written")
        self.assertIsNone(a.sync())
        path = a.sync(final=True)
        self.assertEqual(sorted(a.store.manifest(path)["files"]), ["fresh"])

    def test_a_file_that_never_settles_is_taken_after_max_wait(self):
        a = self.sync("a", 1)
        a.max_wait = 0.3
        a.restore()
        ck = self.root / "a" / "ckpt.pt"
        ck.write_bytes(b"step 1")
        self.assertIsNone(a.sync())  # first seen unsettled: waits
        time.sleep(0.4)
        ck.write_bytes(b"step 2")  # rewritten again: still never settled
        path = a.sync()
        self.assertEqual(sorted(a.store.manifest(path)["files"]), ["ckpt.pt"])
        b = self.sync("b", 2)
        b.restore()
        self.assertEqual((self.root / "b" / "ckpt.pt").read_bytes(), b"step 2")

    def test_stale_attempt_cannot_commit(self):
        a = self.sync("a", 1)
        a.restore()
        (self.root / "a" / "x").write_text("1")
        old(self.root / "a" / "x")
        a.sync()
        b = self.sync("b", 2)
        b.restore()  # attempt 2 claims the run
        (self.root / "a" / "x").write_text("stale")
        old(self.root / "a" / "x")
        self.assertIsNone(a.sync())
        self.assertTrue(a.fenced)
        self.assertEqual(b.store.latest(), {"step": 1, "attempt": 2, "path": "attempt-1/step-1"})

    def test_the_hosts_marker_is_neither_removed_nor_synced(self):
        d = self.root / "a"
        d.mkdir()
        (d / ".cua-last-used").write_text("")  # the host's 7-day clock for this run's data
        a = self.sync("a", 1)
        a.restore()
        self.assertTrue((d / ".cua-last-used").exists())
        (d / "ckpt").write_text("x")
        old(d / "ckpt")
        a.sync()
        self.assertEqual(sorted(_store.scan(d)), ["ckpt"])
        again = self.sync("a", 2)
        again.restore()
        self.assertTrue((d / ".cua-last-used").exists())

    def test_restore_replaces_local_changes_and_removes_extras(self):
        a = self.sync("a", 1)
        a.restore()
        d = self.root / "a"
        (d / "keep").write_text("v1")
        old(d / "keep")
        a.sync()
        (d / "keep").write_text("edited after the snapshot")
        (d / "extra").write_text("not in the snapshot")
        again = self.sync("a", 2)  # the same host, the next attempt
        again.restore()
        self.assertEqual((d / "keep").read_text(), "v1")
        self.assertFalse((d / "extra").exists())

    def test_other_ranks_wait_for_rank0_claim(self):
        a = self.sync("a", 1)
        a.restore()
        (self.root / "a" / "w").write_text("1")
        old(self.root / "a" / "w")
        a.sync()
        r1 = self.sync("r1", 2, rank=1, world=2)
        got = {}
        t = threading.Thread(target=lambda: got.setdefault("doc", r1.restore(wait_secs=30)))
        t.start()
        time.sleep(0.8)
        self.assertNotIn("doc", got)  # attempt 2 has not claimed yet
        self.sync("r0", 2, rank=0, world=2).restore()
        t.join(30)
        self.assertEqual(got["doc"]["attempt"], 2)
        self.assertEqual((self.root / "r1" / "w").read_text(), "1")


@unittest.skipUnless(os.environ.get("CUA_TEST_S3_URL"), "set CUA_TEST_S3_URL=s3://bucket/prefix to test S3")
class DataDirSyncS3(DataDirSyncFile):
    def setUp(self):
        super().setUp()
        self.url = os.environ["CUA_TEST_S3_URL"].rstrip("/") + f"/d-{uuid.uuid4().hex[:10]}"
        self.addCleanup(self._wipe)

    def _wipe(self):
        b = _store.open_backend(self.url)
        for k in b.list(""):
            b.delete(k)


class Liveness(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.hb = Path(self.tmp.name) / "hb"
        os.environ["CUA_HEARTBEAT"] = str(self.hb)
        self.addCleanup(os.environ.pop, "CUA_HEARTBEAT", None)
        progress.explicit = False
        progress._last = 0.0

    def test_tree_cpu_counts_children(self):
        before = _cua_entry.tree_cpu_seconds()
        self.assertIsNotNone(before)
        p = subprocess.Popen([sys.executable, "-c", "import time\nt=time.time()\nwhile time.time()-t<1.5: pass"])
        try:
            time.sleep(1.2)
            during = _cua_entry.tree_cpu_seconds()
        finally:
            p.wait(10)
        self.assertGreater(during - before, 0.5)

    def test_parse_cputime(self):
        self.assertEqual(_cua_entry._parse_cputime("0:01.50"), 1.5)
        self.assertEqual(_cua_entry._parse_cputime("1:02:03"), 3723)
        self.assertEqual(_cua_entry._parse_cputime("2-00:00:01"), 172801)

    def test_cpu_time_beats_while_silent(self):
        live = _cua_entry.Liveness(cpu_every=0.2, cpu_min=0.05)
        t = threading.Thread(target=live._cpu, daemon=True)
        t.start()
        t0 = time.time()
        while time.time() - t0 < 1.0:
            pass  # silent, busy
        live._stop.set()
        t.join(5)
        doc = json.loads(self.hb.read_text())
        self.assertEqual(doc["step"], 0)
        self.assertGreater(doc["t_ms"], (t0 - 1) * 1000)

    def test_explicit_steps_win(self):
        progress.step(41)
        self.assertEqual(json.loads(self.hb.read_text())["step"], 41)
        _cua_entry.Liveness().beat(force=True)  # ignored after an explicit report
        self.assertEqual(json.loads(self.hb.read_text())["step"], 41)


JOB = r'''
import os, signal, subprocess, sys, time
import cua
from cua import checkpoint

app = cua.App("t")

@app.function(nodes=1, store=os.environ.get("STORE_URL"))
def train(cluster: cua.Cluster):
    last = checkpoint.latest(cluster.data_dir)
    start = last.step if last else 0
    if last:
        print(f"resumed from step {start}: {(last.path / 'w').read_text()}", flush=True)
    for step in range(start + 10, 41, 10):
        checkpoint.save(cluster.data_dir, step, {"w": f"step{step}-attempt{cluster.attempt}".encode()})
        print(f"step {step}", flush=True)
        if step == int(os.environ.get("DIE_AT", "0")):
            raise SystemExit(3)
        if step == int(os.environ.get("HANG_AT", "0")):
            print("hanging", flush=True)
            time.sleep(60)
    out = cluster.data_dir / "final.txt"
    out.write_text(f"done at {cluster.attempt}")
    return out

@app.function()
def chatty():
    # a child process prints; its output counts as progress too
    subprocess.run([sys.executable, "-c", "print('iter 12: loss 1.0', flush=True)"], check=True)
'''


class Entry(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        (self.root / "job.py").write_text(JOB)
        self.url = "file://" + str(self.root / "store")

    def env(self, host, attempt, **extra):
        return {**os.environ, "PYTHONPATH": str(RUNTIME), "STORE_URL": self.url, "CUA_RUN_ID": "e2e",
                "CUA_ATTEMPT": str(attempt), "CUA_JOB_DATA": str(self.root / host),
                "CUA_RESULT_PATH": str(self.root / f"result-{host}"),
                "CUA_HEARTBEAT": str(self.root / f"hb-{host}"), **extra}

    def run_job(self, host, attempt, fn="train", **extra):
        return subprocess.run([sys.executable, "-m", "_cua_entry", "job.py", fn], cwd=self.root,
                              env=self.env(host, attempt, **extra), capture_output=True, text=True, timeout=60)

    def latest(self):
        return json.loads((self.root / "store" / "e2e" / "LATEST").read_text())

    def test_kill_then_resume_on_a_new_host(self):
        spec = subprocess.run([sys.executable, "-m", "_cua_entry", "--spec", "job.py", "train"], cwd=self.root,
                              env=self.env("x", 1), capture_output=True, text=True, timeout=60)
        self.assertEqual(json.loads(spec.stdout)["env"]["CUA_CHECKPOINT_URL"], self.url)
        self.assertEqual(json.loads(spec.stdout)["store"], self.url)
        r1 = self.run_job("mac-a", 1, DIE_AT="20")
        self.assertEqual(r1.returncode, 3, r1.stderr)
        self.assertIn("data_dir synced", r1.stdout)  # the final sync ran although the job failed
        r2 = self.run_job("aws-1", 2)
        self.assertEqual(r2.returncode, 0, r2.stderr)
        self.assertIn("data_dir restored from snapshot", r2.stdout)
        self.assertIn("resumed from step 20: step20-attempt1", r2.stdout)
        self.assertEqual(self.latest()["attempt"], 2)
        self.assertEqual(json.loads((self.root / "hb-aws-1").read_text())["step"], 40)
        import tarfile
        with tarfile.open(self.root / "result-aws-1") as t:
            self.assertEqual(t.extractfile("final.txt").read(), b"done at 2")
        # The old attempt comes back (a cut-off host): it does not even start.
        before = self.latest()
        r3 = self.run_job("mac-a", 1)
        self.assertEqual(r3.returncode, 1)
        self.assertIn("this attempt was superseded", r3.stdout)
        self.assertEqual(self.latest(), before)

    def test_sigterm_syncs_before_exit(self):
        p = subprocess.Popen([sys.executable, "-m", "_cua_entry", "job.py", "train"], cwd=self.root,
                             env=self.env("h", 1, HANG_AT="10"), stdout=subprocess.PIPE, text=True)
        try:
            deadline = time.monotonic() + 30
            while not (self.root / "h" / "ckpt-10").exists():
                self.assertLess(time.monotonic(), deadline)
                time.sleep(0.05)
            time.sleep(0.5)
            p.send_signal(signal.SIGTERM)
            out, _ = p.communicate(timeout=30)
        finally:
            if p.poll() is None:
                p.kill()
        self.assertEqual(p.returncode, 128 + signal.SIGTERM)
        self.assertIn("data_dir synced", out)
        r = self.run_job("other", 2)
        self.assertIn("resumed from step 10: step10-attempt1", r.stdout)

    def test_child_output_is_progress(self):
        r = self.run_job("c", 1, fn="chatty", STORE_URL="")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("iter 12: loss 1.0", r.stdout)  # passed through
        self.assertEqual(json.loads((self.root / "hb-c").read_text())["step"], 12)

    def test_entry_reports_ready_and_waits_for_go(self):
        (self.root / "b.py").write_text("import cua\napp = cua.App('b')\n@app.function()\n"
                                        "def f():\n    print('user code runs', flush=True)\n")
        bar = self.root / "barrier"
        env = {**os.environ, "PYTHONPATH": str(RUNTIME), "CUA_BARRIER": str(bar),
               "CUA_BARRIER_TIMEOUT": "30", "CUA_JOB_DATA": str(self.root / "data")}
        p = subprocess.Popen([sys.executable, "-m", "_cua_entry", "b.py", "f"], cwd=self.root, env=env,
                             stdout=subprocess.PIPE, text=True)
        try:
            deadline = time.monotonic() + 30
            while not (bar / "ready").exists():
                self.assertLess(time.monotonic(), deadline)
                time.sleep(0.05)
            time.sleep(0.3)
            self.assertIsNone(p.poll())  # held at the barrier
            (bar / "go").write_text("go")
            out, _ = p.communicate(timeout=30)
        finally:
            if p.poll() is None:
                p.kill()
        self.assertEqual(p.returncode, 0)
        self.assertIn("user code runs", out)


class Barrier(unittest.TestCase):
    def test_waits_for_go(self):
        with tempfile.TemporaryDirectory() as d:
            bar = Path(d) / "barrier"

            def host():
                while not (bar / "ready").exists():
                    time.sleep(0.02)
                time.sleep(0.3)
                (bar / "go").write_text("go")

            t = threading.Thread(target=host)
            t.start()
            t0 = time.monotonic()
            _cua_entry.wait_barrier(str(bar), 0, 30)
            t.join()
            self.assertGreaterEqual(time.monotonic() - t0, 0.25)
            self.assertEqual(json.loads((bar / "ready").read_text())["rank"], 0)

    def test_abort(self):
        with tempfile.TemporaryDirectory() as d:
            bar = Path(d) / "barrier"
            bar.mkdir()
            (bar / "abort").write_text("barrier timed out after 900s (rank 1 not ready)")
            with self.assertRaises(SystemExit) as cm:
                _cua_entry.wait_barrier(str(bar), 1)
            self.assertIn("rank 1 not ready", str(cm.exception))

    def test_local_limit(self):
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(RuntimeError):
                _cua_entry.wait_barrier(str(Path(d) / "b"), 0, timeout=-59.7)  # guard = timeout + 60 s

    def test_no_barrier_is_noop(self):
        _cua_entry.wait_barrier(None, 0)


if __name__ == "__main__":
    unittest.main()

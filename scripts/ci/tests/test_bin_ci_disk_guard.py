"""Tests for bin/ci's disk guard: preflight skip, watch thresholds, lean env, target dir, per-crate commands."""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
from test_bin_ci import ci  # noqa: E402


class Thresholds(unittest.TestCase):
    def test_defaults(self):
        with mock.patch.dict(os.environ, {}, clear=True):
            self.assertEqual((ci.min_free_gb(), ci.floor_free_gb(), ci.max_target_gb()), (40.0, 25.0, 15.0))

    def test_env_override_and_garbage(self):
        with mock.patch.dict(os.environ, {"CUA_CI_MIN_FREE_GB": "60", "CUA_CI_MAX_TARGET_GB": "x"}, clear=True):
            self.assertEqual(ci.min_free_gb(), 60.0)
            self.assertEqual(ci.max_target_gb(), 15.0)


class Preflight(unittest.TestCase):
    def test_low_disk_skips(self):
        why = ci.preflight_reason(13, 40)
        self.assertIn("skipped: low disk", why)
        self.assertIn("CI will run these", why)

    def test_enough_disk_runs(self):
        self.assertIsNone(ci.preflight_reason(40, 40))
        self.assertIsNone(ci.preflight_reason(500, 40))

    def test_fake_free_value(self):
        with mock.patch.dict(os.environ, {"CUA_CI_FAKE_FREE_GB": "13"}):
            self.assertEqual(ci.disk_free_gb(Path("/nonexistent/x")), 13.0)


class Watch(unittest.TestCase):
    def test_ok(self):
        self.assertIsNone(ci.watch_reason(100, 5, 25, 15))

    def test_target_over_cap(self):
        self.assertIn("over the 15 GB cap", ci.watch_reason(100, 15.5, 25, 15))

    def test_free_under_floor(self):
        self.assertIn("under the 25 GB floor", ci.watch_reason(24.9, 1, 25, 15))

    def test_boundaries_are_inclusive_of_safe(self):
        self.assertIsNone(ci.watch_reason(25, 15, 25, 15))


class Guard(unittest.TestCase):
    def test_trip_kills_running_process(self):
        with tempfile.TemporaryDirectory() as d:
            guard = ci.DiskGuard(Path(d), interval=3600)
            proc = subprocess.Popen(["sleep", "60"], start_new_session=True)
            guard.register(proc)
            with mock.patch.dict(os.environ, {"CUA_CI_FAKE_FREE_GB": "10"}):
                reason = guard.check_once()
            self.assertIn("floor", reason)
            self.assertEqual(guard.tripped, reason)
            self.assertIsNotNone(proc.wait(timeout=10))
            self.assertNotEqual(proc.returncode, 0)

    def test_no_trip_leaves_process(self):
        with tempfile.TemporaryDirectory() as d:
            guard = ci.DiskGuard(Path(d), interval=3600)
            proc = subprocess.Popen(["sleep", "60"], start_new_session=True)
            guard.register(proc)
            try:
                with mock.patch.dict(os.environ, {"CUA_CI_FAKE_FREE_GB": "500"}):
                    self.assertIsNone(guard.check_once())
                self.assertIsNone(proc.poll())
            finally:
                proc.kill()
                proc.wait()

    def test_target_cap_uses_dir_size(self):
        with tempfile.TemporaryDirectory() as d:
            (Path(d) / "big").write_bytes(b"x" * 2_000_000)
            guard = ci.DiskGuard(Path(d), interval=3600)
            with mock.patch.dict(os.environ, {"CUA_CI_FAKE_FREE_GB": "500", "CUA_CI_MAX_TARGET_GB": "0.001"}):
                self.assertIn("cap", guard.check_once())


class Lean(unittest.TestCase):
    def test_defaults_set(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(ci, "have", return_value=False):
            env = ci.lean_env()
        self.assertEqual(env["CARGO_INCREMENTAL"], "0")
        self.assertEqual(env["CARGO_PROFILE_DEV_DEBUG"], "0")
        self.assertEqual(env["CARGO_PROFILE_TEST_DEBUG"], "0")
        self.assertNotIn("RUSTC_WRAPPER", env)

    def test_sccache_when_present(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(ci, "have", return_value=True):
            self.assertEqual(ci.lean_env()["RUSTC_WRAPPER"], "sccache")

    def test_user_env_wins(self):
        with mock.patch.dict(os.environ, {"CARGO_INCREMENTAL": "1"}, clear=True), mock.patch.object(ci, "have", return_value=False):
            self.assertNotIn("CARGO_INCREMENTAL", ci.lean_env())


class TargetDir(unittest.TestCase):
    def test_per_run_dir_is_deleted(self):
        with mock.patch.dict(os.environ, {}, clear=True):
            path, delete = ci.run_target_dir(False)
        self.assertTrue(delete)
        self.assertEqual(path.parent, ci.ROOT)
        self.assertIn(str(os.getpid()), path.name)

    def test_keep_target(self):
        with mock.patch.dict(os.environ, {}, clear=True):
            path, delete = ci.run_target_dir(True)
        self.assertFalse(delete)

    def test_user_target_never_deleted(self):
        with mock.patch.dict(os.environ, {"CARGO_TARGET_DIR": "/tmp/mine"}):
            self.assertFalse(ci.run_target_dir(False)[1])


class PerCrate(unittest.TestCase):
    def test_single(self):
        self.assertEqual(ci.per_crate("cargo test {p}", ["a"], " -- x"), "cargo test -p a -- x")

    def test_many_run_one_at_a_time_and_fail_if_any_failed(self):
        cmd = ci.per_crate("cargo test {p}", ["a", "b"], " -- x")
        self.assertEqual(cmd, "rc=0; cargo test -p a -- x || rc=1; cargo test -p b -- x || rc=1; exit $rc")
        self.assertEqual(subprocess.run([ci.BASH, "-c", ci.per_crate("{p}", ["true", "false"]).replace("-p ", "")]).returncode, 1)
        self.assertEqual(subprocess.run([ci.BASH, "-c", ci.per_crate("{p}", ["true", "true"]).replace("-p ", "")]).returncode, 0)


class Plan(unittest.TestCase):
    def test_heavy_classification(self):
        self.assertTrue(ci.is_heavy(ci.Check("a", "a", "true", tier="build")))
        self.assertTrue(ci.is_heavy(ci.Check("a", "a", "true", heavy=True)))
        self.assertFalse(ci.is_heavy(ci.Check("a", "a", "true")))


class EndToEnd(unittest.TestCase):
    def test_low_disk_skips_heavy_checks_and_quick_still_runs(self):
        env = {**os.environ, "CUA_CI_FAKE_FREE_GB": "13", "CUA_CI_ROOT": str(ci.ROOT)}
        out = subprocess.run([sys.executable, str(ci.ROOT / "bin/ci"), "--full", "--only", "rust-clippy,rust-test", "--no-install", "--base", "HEAD"],
                             capture_output=True, text=True, env=env, cwd=str(ci.ROOT), timeout=300)
        text = out.stdout + out.stderr
        if "skipped: low disk" not in text and "no local check" not in text and "cargo not installed" not in text:
            self.fail(text)
        self.assertNotIn("FAIL", text)


if __name__ == "__main__":
    unittest.main()

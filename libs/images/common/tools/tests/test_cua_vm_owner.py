"""Tests for common/tools/cua-vm-owner (python3 -m unittest discover).

A fake `lume` keeps VMs as directories under a temp root, so nothing here
touches a real VM.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
TOOL = os.path.join(HERE, "..", "cua-vm-owner")

FAKE_LUME = """#!/bin/sh
cmd="$1"; vm="$2"
case "$cmd" in
  get) [ -d "$FAKE_LUME_ROOT/$vm" ] || exit 1
       printf '{"name":"%s","locationName":"home","status":"stopped"}\\n' "$vm" ;;
  stop) exit 0 ;;
  delete) [ -d "$FAKE_LUME_ROOT/$vm" ] || exit 1
          rm -rf "$FAKE_LUME_ROOT/$vm"; echo "$vm" >>"$FAKE_LUME_ROOT/../deleted" ;;
  *) exit 2 ;;
esac
"""


def dead_pid() -> int:
    p = subprocess.Popen(["true"])
    p.wait()
    return p.pid


class VmOwnerTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.root = os.path.join(self.tmp, "lume")
        os.makedirs(self.root)
        self.ledger = os.path.join(self.tmp, "ledger")
        fake = os.path.join(self.tmp, "fake-lume")
        with open(fake, "w") as f:
            f.write(FAKE_LUME)
        os.chmod(fake, 0o755)
        self.env = dict(
            os.environ,
            CUA_VM_LEDGER=self.ledger,
            CUA_VM_OWNER_LUME=fake,
            CUA_LUME_ROOT=self.root,
            FAKE_LUME_ROOT=self.root,
        )
        self.procs = []

    def tearDown(self):
        for p in self.procs:
            p.kill()
            p.wait()
        shutil.rmtree(self.tmp)

    def run_tool(self, *args, owner=None):
        env = dict(self.env)
        if owner is not None:
            env["CUA_VM_OWNER_PID"] = str(owner)
        return subprocess.run(
            [sys.executable, TOOL, *args], env=env, capture_output=True, text=True
        )

    def vm(self, name):
        os.makedirs(os.path.join(self.root, name))

    def exists(self, name):
        return os.path.isdir(os.path.join(self.root, name))

    def mark(self, name, owner, hours_ago=0.0, kind="image-build"):
        r = self.run_tool("mark", name, "--kind", kind, "--run", "r1", owner=owner)
        self.assertEqual(r.returncode, 0, r.stderr)
        for n in os.listdir(self.ledger):
            p = os.path.join(self.ledger, n)
            with open(p) as f:
                rec = json.load(f)
            if rec["vm"] == name:
                rec["created_at"] = int(time.time() - hours_ago * 3600)
                with open(p, "w") as f:
                    json.dump(rec, f)

    def live_pid(self):
        p = subprocess.Popen(["sleep", "60"])
        self.procs.append(p)
        return p.pid

    def test_a_crashed_builds_vm_is_reaped_by_the_next_build(self):
        self.vm("cua-e2e-imgbuild-x-slim")
        self.mark("cua-e2e-imgbuild-x-slim", dead_pid(), hours_ago=3)
        dry = self.run_tool("reap", "--dry-run")
        self.assertIn("would delete cua-e2e-imgbuild-x-slim", dry.stderr)
        self.assertTrue(self.exists("cua-e2e-imgbuild-x-slim"))
        r = self.run_tool("reap", "--kind", "image-build")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("deleted cua-e2e-imgbuild-x-slim", r.stderr)
        self.assertFalse(self.exists("cua-e2e-imgbuild-x-slim"))
        self.assertEqual(os.listdir(self.ledger), [])

    def test_a_vm_without_a_record_is_never_touched_even_with_a_test_name(self):
        for n in ("cua-e2e-imgbuild-y-slim", "cua-ci-1-1-slim", "space-0123456789"):
            self.vm(n)
        r = self.run_tool("reap", "--grace", "0")
        self.assertEqual(r.returncode, 0, r.stderr)
        r = self.run_tool("reclaim", "cua-e2e-imgbuild-y-slim")
        self.assertEqual(r.returncode, 1)
        self.assertIn("not created by us", r.stderr)
        for n in ("cua-e2e-imgbuild-y-slim", "cua-ci-1-1-slim", "space-0123456789"):
            self.assertTrue(self.exists(n), n)
        self.assertFalse(os.path.exists(os.path.join(self.tmp, "deleted")))

    def test_a_vm_re_made_under_a_recorded_name_is_not_ours(self):
        self.vm("cua-e2e-z-slim")
        self.mark("cua-e2e-z-slim", dead_pid(), hours_ago=5)
        shutil.rmtree(os.path.join(self.root, "cua-e2e-z-slim"))
        time.sleep(0.02)
        self.vm("cua-e2e-z-slim")  # someone else's VM now
        r = self.run_tool("reap", "--grace", "0")
        self.assertIn("not the VM the record was written for", r.stderr)
        self.assertTrue(self.exists("cua-e2e-z-slim"))
        self.assertEqual(os.listdir(self.ledger), [])

    def test_a_running_builds_vm_is_not_reaped_by_a_concurrent_build(self):
        self.vm("cua-e2e-busy-slim")
        self.mark("cua-e2e-busy-slim", self.live_pid(), hours_ago=5)
        r = self.run_tool("reap")
        self.assertIn("still going", r.stderr)
        self.assertTrue(self.exists("cua-e2e-busy-slim"))
        r = self.run_tool("reclaim", "cua-e2e-busy-slim")
        self.assertEqual(r.returncode, 1)
        self.assertTrue(self.exists("cua-e2e-busy-slim"))

    def test_an_abandoned_vm_is_reaped_only_after_the_grace_period(self):
        self.vm("cua-ci-9-1-job")
        self.mark("cua-ci-9-1-job", dead_pid(), hours_ago=1, kind="ci")
        r = self.run_tool("reap", "--kind", "ci", "--grace", "2h")
        self.assertIn("kept cua-ci-9-1-job", r.stderr)
        self.assertTrue(self.exists("cua-ci-9-1-job"))
        # Another kind's reaper never looks at it.
        self.run_tool("reap", "--kind", "image-build", "--grace", "0")
        self.assertTrue(self.exists("cua-ci-9-1-job"))
        r = self.run_tool("reap", "--kind", "ci", "--grace", "30m")
        self.assertIn("deleted cua-ci-9-1-job", r.stderr)
        self.assertFalse(self.exists("cua-ci-9-1-job"))

    def test_an_adopted_vm_is_never_reaped_and_reclaim_takes_a_dead_runs_vm(self):
        self.vm("cua-e2e-keep-slim")
        self.mark("cua-e2e-keep-slim", dead_pid(), hours_ago=9)
        self.run_tool("adopt", "cua-e2e-keep-slim")
        self.run_tool("reap", "--grace", "0")
        self.assertTrue(self.exists("cua-e2e-keep-slim"))
        self.vm("cua-e2e-rerun-slim")
        self.mark("cua-e2e-rerun-slim", dead_pid())
        self.assertEqual(self.run_tool("owns", "cua-e2e-rerun-slim").returncode, 0)
        r = self.run_tool("reclaim", "cua-e2e-rerun-slim")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertFalse(self.exists("cua-e2e-rerun-slim"))


if __name__ == "__main__":
    unittest.main()

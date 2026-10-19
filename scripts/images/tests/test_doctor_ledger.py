"""Tests for the image-doctor ledger (python3 -m unittest)."""

from __future__ import annotations

import json
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))
import doctor_ledger  # noqa: E402

D1 = "sha256:" + "1" * 64
D2 = "sha256:" + "2" * 64
IDX = "sha256:" + "9" * 64


def report(status: str = "pass", runtime: str = "gvisor") -> dict:
    return {"schema_version": 1, "producer": "cua-spacesd", "spacesd": {"version": "0.1.0"},
            "image": {"variant": "rootfs", "os": "linux"}, "environment": {"runtime": runtime, "arch": "arm64"},
            "summary": {"status": status, "pass": 1, "warn": 0, "fail": 0 if status == "pass" else 1,
                        "skip": 0, "strict": True}, "checks": [], "fidelity": {}}


class LedgerTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.ledger = self.tmp.name

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def test_record_status_and_regress(self) -> None:
        doctor_ledger.record(self.ledger, "ghcr.io/trycua/linux", D1, report(), {"lane": "runsc"}, "r@x", "run")
        doctor_ledger.record(self.ledger, "ghcr.io/trycua/linux", D1, report(runtime="container"), None, "", "")
        self.assertEqual(doctor_ledger.status(self.ledger, "ghcr.io/trycua/linux", D1, [])[0], "pass")
        self.assertEqual(doctor_ledger.status(self.ledger, "ghcr.io/trycua/linux", D1, ["runsc", "runc"])[0], "pass")
        self.assertEqual(doctor_ledger.status(self.ledger, "ghcr.io/trycua/linux", D1, ["qemu"])[0], "missing")
        doctor_ledger.regress(self.ledger, "ghcr.io/trycua/linux", D1, "runsc", "nightly")
        self.assertEqual(doctor_ledger.status(self.ledger, "ghcr.io/trycua/linux", D1, [])[0], "regressed")
        self.assertEqual(doctor_ledger.status(self.ledger, "ghcr.io/trycua/linux", D2, [])[0], "missing")

    def test_failed_lane_fails_the_entry(self) -> None:
        doctor_ledger.record(self.ledger, "ghcr.io/trycua/linux", D1, report("fail"), {"lane": "qemu", "claim_secrets": True}, "", "")
        verdict, detail = doctor_ledger.status(self.ledger, "ghcr.io/trycua/linux", D1, [])
        self.assertEqual(verdict, "fail")
        self.assertIn("qemu-claim-secrets=fail", detail)

    def test_bad_inputs_are_refused(self) -> None:
        with self.assertRaises(ValueError):
            doctor_ledger.entry_path(self.ledger, "ghcr.io/trycua/linux", "latest")
        with self.assertRaises(ValueError):
            doctor_ledger.record(self.ledger, "ghcr.io/trycua/linux", D1, {"schema_version": 2}, None, "", "")

    def test_summary_table(self) -> None:
        lane = os.path.join(self.ledger, "reports", "doctor-x-arm64-runsc")
        os.makedirs(lane)
        with open(os.path.join(lane, "report.json"), "w") as fh:
            json.dump(report(), fh)
        md = doctor_ledger.summary(os.path.join(self.ledger, "reports"))
        self.assertIn("| runsc | arm64 |", md)
        self.assertIn("**pass**", md)



class MergeTests(unittest.TestCase):
    def test_lanes_union_and_ours_win_per_lane(self) -> None:
        with tempfile.TemporaryDirectory() as a, tempfile.TemporaryDirectory() as b:
            repo = "ghcr.io/x/linux"
            doctor_ledger.record(b, repo, D1, report("pass", "gvisor"), None, "r1", "run-theirs")
            doctor_ledger.record(b, repo, D1, report("pass", "container"), None, "r2", "run-theirs")
            doctor_ledger.record(a, repo, D1, report("fail", "container"), None, "r3", "run-ours")
            doctor_ledger.record(a, repo, D2, report("pass", "gvisor"), None, "r4", "run-ours")
            with open(os.path.join(a, "README.md"), "w") as fh:
                fh.write("not an entry\n")
            merged = doctor_ledger.merge(a, b)
            self.assertEqual(len(merged), 2)
            e1 = doctor_ledger.read_entry(b, repo, D1)
            self.assertEqual(sorted(e1["lanes"]), ["runc", "runsc"])
            self.assertEqual(e1["lanes"]["runc"]["workflow_run"], "run-ours")
            self.assertEqual(e1["lanes"]["runsc"]["workflow_run"], "run-theirs")
            self.assertEqual(e1["status"], "fail")
            self.assertEqual(doctor_ledger.read_entry(b, repo, D2)["status"], "pass")

if __name__ == "__main__":
    unittest.main()

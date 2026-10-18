"""Tests for verify_pins.py against the published 24.04-slim disk index
(captured with crane; report refs moved to a placeholder namespace) and
its doctor-attest record (lane verdicts only)."""

from __future__ import annotations

import copy
import json
import os
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))
import verify_pins  # noqa: E402

FIX = os.path.join(HERE, "fixtures")
AMD64_DISK = "sha256:c015b11ed49737f4533cb44a3af620d59181f7ffceef7f6b4ee85c79eff4a8a3"
ARM64_DISK = "sha256:bce21bf479017ff409845fac7a6aab720d320ae443a49959eee4b9f1a2b52bb5"


def load(name: str):
    with open(os.path.join(FIX, name)) as fh:
        return json.load(fh)


class VerifyPinsTests(unittest.TestCase):
    def setUp(self) -> None:
        self.index = load("linux-24.04-slim-disk-index.json")
        self.attested = load("linux-24.04-slim-attested.json")

    def test_doctored_child_passes(self) -> None:
        self.assertEqual(verify_pins.check_child(self.index, "amd64", "containerdisk", AMD64_DISK, self.attested), AMD64_DISK)

    def test_undoctored_arm64_disk_passes_when_no_lane_ran(self) -> None:
        # The case that failed run 36208119058: hosted arm64 has no KVM, so
        # the arm64 disk has no doctor lane and no annotation.
        self.assertEqual(verify_pins.check_child(self.index, "arm64", "containerdisk", ARM64_DISK, self.attested), ARM64_DISK)

    def test_missing_annotation_fails_when_a_lane_ran(self) -> None:
        attested = self.attested + [{"arch": "arm64", "lane": "qemu", "variant": "containerdisk", "status": "pass"}]
        with self.assertRaises(ValueError):
            verify_pins.check_child(self.index, "arm64", "containerdisk", ARM64_DISK, attested)

    def test_wrong_digest_or_failed_verdict_fails(self) -> None:
        with self.assertRaises(ValueError):
            verify_pins.check_child(self.index, "arm64", "containerdisk", AMD64_DISK, self.attested)
        failed = copy.deepcopy(self.index)
        failed["manifests"][0]["annotations"]["ai.cua.doctor.status"] = "fail"
        with self.assertRaises(ValueError):
            verify_pins.check_child(failed, "amd64", "containerdisk", AMD64_DISK, self.attested)
        with self.assertRaises(ValueError):
            verify_pins.check_child(self.index, "riscv64", "containerdisk", AMD64_DISK, self.attested)


if __name__ == "__main__":
    unittest.main()

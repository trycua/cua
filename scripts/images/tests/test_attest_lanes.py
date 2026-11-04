"""attest-doctor-reports.py lanes: which doctor reports attach to which pushed child."""

from __future__ import annotations

import importlib.util
import json
import os
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SPEC = importlib.util.spec_from_file_location("attest", os.path.join(HERE, "..", "attest-doctor-reports.py"))
attest = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(attest)


def write(path: str, data: dict) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as fh:
        json.dump(data, fh)


class LanesTests(unittest.TestCase):
    def test_windows_evidence_attaches_to_the_disk_child(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            # windows-2022 release-push.sh writes both: the summary record
            # (no arch) must not be read as a per-arch push record.
            write(f"{d}/pushed.json", {"stamp": "20260926-abcdef1", "disk_digest": "sha256:i", "primary_digest": "sha256:p"})
            write(f"{d}/pushed-amd64/pushed.json", {"arch": "amd64", "repo": "ghcr.io/trycua/windows", "containerdisk": "sha256:c"})
            for lane_dir in ("doctor/amd64-windows", "doctor/amd64-windows-pushed"):
                write(f"{d}/{lane_dir}/report.json", {"schema_version": 1})
                write(f"{d}/{lane_dir}/lane.json", {"lane": "windows", "arch": "amd64", "variant": "containerdisk"})
            rows = attest.lanes(d)
        self.assertEqual(len(rows), 2)
        self.assertTrue(all(r[5] == "ghcr.io/trycua/windows@sha256:c" for r in rows))

    def test_a_report_without_a_push_record_fails(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            write(f"{d}/doctor/arm64-qemu/report.json", {"schema_version": 1})
            write(f"{d}/doctor/arm64-qemu/lane.json", {"lane": "qemu", "arch": "arm64", "variant": "containerdisk"})
            with self.assertRaises(SystemExit):
                attest.lanes(d)


if __name__ == "__main__":
    unittest.main()

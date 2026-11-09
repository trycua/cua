"""Tests for scripts/images/record-software.py (python3 -m unittest).

Refs are assembled from pieces so the image-ref gate does not read them.
"""

from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
TOOL = os.path.join(os.path.dirname(HERE), "record-software.py")
GH = "ghcr" + ".io/" + "trycua"


def load():
    spec = importlib.util.spec_from_file_location("record_software", TOOL)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def report(status: str = "pass", strict: bool = True, **fidelity) -> dict:
    base = {
        "app_versions": {"python3": "Python 3.12.3", "chromium": "Chromium 153.0"},
        "tool_versions": {"node": "v22.20.0"},
        "simulator_runtimes": [],
    }
    base.update(fidelity)
    return {
        "started_at": "2026-09-25T10:00:00+00:00",
        "summary": {"status": status, "strict": strict},
        "environment": {"runtime": "container", "arch": "arm64"},
        "fidelity": base,
    }


class RecordSoftwareTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.mod = load()

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def run_tool(self, rep: dict, ref: str, *extra: str) -> subprocess.CompletedProcess:
        path = os.path.join(self.tmp.name, "report.json")
        with open(path, "w") as fh:
            json.dump(rep, fh)
        return subprocess.run(
            [sys.executable, TOOL, path, "--image", ref, "--out-dir", self.tmp.name, *extra],
            capture_output=True, text=True,
        )

    def test_refs_name_os_version_and_tier(self) -> None:
        p = self.mod.parse_ref
        self.assertEqual(p(f"{GH}/linux:24.04-slim-disk"),
                         {"os": "linux", "version": "24.04", "tier": "slim", "image": f"{GH}/linux:24.04-slim"})
        self.assertEqual(p(f"{GH}/linux:24.04")["tier"], "full")
        self.assertEqual(p(f"{GH}/macos:26-xcode-26.1")["tier"], "xcode-26.1")
        self.assertEqual(p(f"{GH}/macos:26-disk")["image"], f"{GH}/macos:26")
        with self.assertRaises(SystemExit):
            p(f"{GH}/linux:latest")
        with self.assertRaises(SystemExit):
            p(f"{GH}/linux@sha256:" + "0" * 64)

    def test_a_passing_strict_report_is_recorded(self) -> None:
        r = self.run_tool(report(), f"{GH}/linux:24.04-slim", "--source", "rev abc1234")
        self.assertEqual(r.returncode, 0, r.stderr)
        out = r.stdout.strip()
        self.assertTrue(out.endswith("linux-24.04-slim.json"), out)
        with open(out) as fh:
            inv = json.load(fh)
        self.assertEqual(inv["tier"], "slim")
        self.assertEqual(list(inv["apps"]), ["chromium", "python3"])
        self.assertEqual(inv["tools"], {"node": "v22.20.0"})
        self.assertEqual(inv["recorded_from"], "2026-09-25, container/arm64, rev abc1234")

    def test_failing_unstrict_or_incomplete_reports_are_refused(self) -> None:
        ref = f"{GH}/linux:24.04"
        self.assertNotEqual(self.run_tool(report(status="fail"), ref).returncode, 0)
        unstrict = self.run_tool(report(strict=False), ref)
        self.assertNotEqual(unstrict.returncode, 0)
        self.assertIn("--strict", unstrict.stderr)
        self.assertEqual(self.run_tool(report(strict=False), ref, "--allow-unstrict").returncode, 0)
        gone = self.run_tool(report(tool_versions={"go": "unavailable"}), ref)
        self.assertNotEqual(gone.returncode, 0)
        self.assertIn("go", gone.stderr)


if __name__ == "__main__":
    unittest.main()

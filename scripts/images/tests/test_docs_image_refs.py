"""Tests for the docs image gate (python3 -m unittest)."""

from __future__ import annotations

import json
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))
import doctor_ledger  # noqa: E402
import importlib.util  # noqa: E402

spec = importlib.util.spec_from_file_location("check_docs", os.path.join(os.path.dirname(HERE), "check-docs-image-refs.py"))
check_docs = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check_docs)

D1 = "sha256:" + "1" * 64
D2 = "sha256:" + "2" * 64
IDX = "sha256:" + "9" * 64


def report(status: str = "pass", runtime: str = "gvisor") -> dict:
    return {"schema_version": 1, "producer": "cua-spacesd", "spacesd": {"version": "0.1.0"},
            "image": {"variant": "rootfs", "os": "linux"}, "environment": {"runtime": runtime, "arch": "arm64"},
            "summary": {"status": status, "pass": 1, "warn": 0, "fail": 0 if status == "pass" else 1,
                        "skip": 0, "strict": True}, "checks": [], "fidelity": {}}


class DocsGateTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        root = self.tmp.name
        os.makedirs(os.path.join(root, "docs"))
        with open(os.path.join(root, "docs", "guide.mdx"), "w") as fh:
            fh.write(
                "Run `ghcr.io/trycua/linux:24.04` or the disk ghcr.io/trycua/linux:24.04-disk.\n"
                "See github.com/trycua/cua and trycua/cua (a repo, not an image).\n"
                "<!-- cua-image-unverified -->\n"
                "docker run ghcr.io/trycua/example:1.0\n"
                "Template ghcr.io/trycua/linux:<tag> is not a ref.\n"
                "Hub image trycua/winarena:latest.\n"
            )
        self.root = root
        self.ledger = os.path.join(root, "ledger")
        self.baseline = os.path.join(root, "baseline.json")
        self.write_baseline({})

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def write_baseline(self, refs: dict) -> None:
        with open(self.baseline, "w") as fh:
            json.dump({"refs": refs}, fh)

    def resolved(self) -> str:
        path = os.path.join(self.root, "resolved.json")
        with open(path, "w") as fh:
            json.dump({
                "ghcr.io/trycua/linux:24.04": {"digest": IDX, "children": [D1, D2]},
                "ghcr.io/trycua/linux:24.04-disk": {"digest": D1, "children": []},
                "docker.io/trycua/winarena:latest": {"error": "not found"},
            }, fh)
        return path

    def run_gate(self, *extra: str) -> int:
        return check_docs.main(["--root", self.root, "--ledger", self.ledger, "--baseline", self.baseline,
                                "--resolved", self.resolved(), *extra])

    def test_historical_plan_and_spec_notes_are_not_scanned(self) -> None:
        notes = os.path.join(self.root, "docs", "superpowers", "plans")
        os.makedirs(notes)
        with open(os.path.join(notes, "2026-01-01-old.md"), "w") as fh:
            fh.write("Pinned ghcr.io/trycua/old@sha256:" + "3" * 64 + " back then.\n")
        refs = check_docs.collect(self.root)
        self.assertFalse(any("trycua/old" in r for r in refs))
        # Guides next to them are still scanned.
        self.assertIn("ghcr.io/trycua/linux:24.04", refs)

    def test_extracts_only_real_refs(self) -> None:
        refs = check_docs.collect(self.root)
        self.assertEqual(sorted(refs), ["docker.io/trycua/winarena:latest", "ghcr.io/trycua/linux:24.04",
                                        "ghcr.io/trycua/linux:24.04-disk"])

    def test_every_child_needs_a_pass(self) -> None:
        self.write_baseline({"docker.io/trycua/winarena:latest": "legacy"})
        self.assertEqual(self.run_gate(), 1)
        doctor_ledger.record(self.ledger, "ghcr.io/trycua/linux", D1, report(), {"lane": "runsc"}, "", "")
        self.assertEqual(self.run_gate(), 1, "D2 has no entry yet")
        doctor_ledger.record(self.ledger, "ghcr.io/trycua/linux", D2, report(), {"lane": "runsc"}, "", "")
        self.assertEqual(self.run_gate(), 0)
        doctor_ledger.regress(self.ledger, "ghcr.io/trycua/linux", D2, "runsc", "nightly")
        self.assertEqual(self.run_gate(), 1, "a regression fails the docs gate")

    def test_baseline_only_shrinks(self) -> None:
        base = os.path.join(self.root, "base.json")
        with open(base, "w") as fh:
            json.dump({"refs": {}}, fh)
        self.write_baseline({"docker.io/trycua/winarena:latest": "legacy",
                             "ghcr.io/trycua/linux:24.04": "x", "ghcr.io/trycua/linux:24.04-disk": "x"})
        self.assertEqual(self.run_gate(), 0)
        self.assertEqual(self.run_gate("--baseline-base", base), 1)
        # A passing ref left in the baseline is stale.
        for d in (D1, D2):
            doctor_ledger.record(self.ledger, "ghcr.io/trycua/linux", d, report(), {"lane": "runsc"}, "", "")
        self.assertEqual(self.run_gate(), 1)
        # An entry for a ref no doc uses any more must go.
        self.write_baseline({"docker.io/trycua/winarena:latest": "legacy", "ghcr.io/trycua/gone:1": "x"})
        self.assertEqual(self.run_gate(), 1)


if __name__ == "__main__":
    unittest.main()

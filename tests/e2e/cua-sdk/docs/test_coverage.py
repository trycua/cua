"""Unit tests for the docs-coverage join (stdlib only)."""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

import coverage
import extract

POLICY = {
    "lanes": {"docs": {"e2e": "docs"}, "terraform": {"e2e": "docs"}, "fleet": {"e2e": "fleet"}},
    "skip": {},
}


class CoverageTest(unittest.TestCase):
    def setUp(self):
        tmp = Path(tempfile.mkdtemp())
        (tmp / "p.mdx").write_text(
            '```python test="docs" id="a"\nx\n```\n'
            '```python test="docs,fleet" id="b"\nx\n```\n'
            '```hcl test="terraform" id="c"\nx\n```\n'
        )
        self.blocks = extract.all_blocks(tmp)

    def row(self, block_id, lane, status):
        return {"block_id": block_id, "lane": lane, "status": status, "test": f"t[{block_id}]"}

    def test_every_block_of_a_ran_lane_needs_a_passing_result(self):
        results = [self.row("p#a", "docs", "pass"), self.row("p#b", "docs", "fail")]
        rows, problems = coverage.join(self.blocks, results, POLICY, {"docs"})
        self.assertEqual(
            [(r["block_id"], r["status"]) for r in rows],
            [("p#a", "pass"), ("p#b", "fail"), ("p#c", "missing")],
        )
        self.assertEqual(len(problems), 2)
        self.assertIn("p#b (docs) failed", problems[0])
        self.assertIn("p#c (terraform) produced no result in lane docs", problems[1])

    def test_lanes_that_did_not_run_are_ignored(self):
        rows, problems = coverage.join(self.blocks, [], POLICY, {"container"})
        self.assertEqual((rows, problems), ([], []))

    def test_skips_and_xfails_are_not_missing(self):
        results = [
            self.row("p#b", "fleet", "skip"),
        ]
        rows, problems = coverage.join(self.blocks, results, POLICY, {"fleet"})
        self.assertEqual([(r["block_id"], r["status"]) for r in rows], [("p#b", "skip")])
        self.assertEqual(problems, [])
        self.assertIn("| p.mdx | 0 | 0 | 1 | 0 | 0 |", coverage.summary(rows))


if __name__ == "__main__":
    unittest.main()

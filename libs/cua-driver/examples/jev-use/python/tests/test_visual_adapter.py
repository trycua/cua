from __future__ import annotations

import json
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "python"))

from visual_adapter import build_visual_candidates


class VisualAdapterTest(unittest.TestCase):
    def test_frozen_visual_region_cases(self) -> None:
        fixture = json.loads((ROOT / "fixtures/visual_regions_v1.json").read_text())
        for case in fixture["cases"]:
            with self.subTest(case=case["name"]):
                candidates = build_visual_candidates(case["observation"], case["visual_regions"])
                self.assertEqual([candidate.id for candidate in candidates], case["expected"])
                executable = [candidate for candidate in candidates if candidate.tool]
                if "expected_arguments" in case:
                    self.assertEqual(len(executable), 1)
                    self.assertEqual(executable[0].arguments, case["expected_arguments"])
                else:
                    self.assertEqual(executable, [])


if __name__ == "__main__":
    unittest.main()

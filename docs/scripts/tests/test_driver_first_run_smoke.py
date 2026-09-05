"""Synthetic result-oracle checks; importing the runner does not install anything."""

import importlib.util
from pathlib import Path
import unittest


spec = importlib.util.spec_from_file_location(
    "driver_first_run", Path(__file__).parents[1] / "verify-driver-first-run-linux.py"
)
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class DisplayResultTests(unittest.TestCase):
    def test_gtk_text_label(self):
        self.assertTrue(runner.displays_42({
            "elements": [{"role": "text", "label": "42", "value": None}]
        }))

    def test_accessible_text_value(self):
        self.assertTrue(runner.displays_42({
            "elements": [{"role": "entry", "label": "Result", "value": "42"}]
        }))

    def test_other_controls_numbers_and_snapshot_indices_do_not_pass(self):
        for element in (
            {"role": "toggle button", "label": "42"},
            {"role": "text", "label": "642"},
            {"role": "text", "label": "7", "element_index": 42},
        ):
            with self.subTest(element=element):
                self.assertFalse(runner.displays_42({
                    "elements": [element], "tree_markdown": '[42] text "7"'
                }))


if __name__ == "__main__":
    unittest.main()

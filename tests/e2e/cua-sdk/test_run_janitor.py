"""The e2e janitor removes only this run's containers (stdlib only).

python3 -m unittest tests/e2e/cua-sdk/test_run_janitor.py
"""

from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

_spec = importlib.util.spec_from_file_location("e2e_run", Path(__file__).with_name("run.py"))
run = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(run)

LISTING = "\n".join(
    [
        "a1 cua-e2e-gh123-1-py-local",
        "a2 /cua-e2e-gh123-1-ts-vm",
        "b1 cua-e2e-gh123-10-py-local",  # another attempt: not this run
        "b2 xcua-e2e-gh123-1-foo",  # not a prefix match
        "b3 readers-desktop",
        "b4 cua-e2e-gh124-1-py",
    ]
)


class Janitor(unittest.TestCase):
    def test_only_this_runs_prefix(self) -> None:
        self.assertEqual(run.janitor_targets("gh123-1", LISTING), ["a1", "a2"])

    def test_refuses_run_ids_that_could_widen_the_match(self) -> None:
        for bad in ["", "a", "abc", "*", "gh123-1 ", "GH123", "../x"]:
            with self.assertRaises(ValueError, msg=bad):
                run.janitor_targets(bad, LISTING)

    def test_nothing_matches_nothing_removed(self) -> None:
        self.assertEqual(run.janitor_targets("abcdef", LISTING), [])


if __name__ == "__main__":
    unittest.main()

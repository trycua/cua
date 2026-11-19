"""Run: python3 .github/scripts/tests/test_feed_monotonic.py"""

import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import feed_monotonic as fm  # noqa: E402


def yml(v):
    return "version: %s\nfiles:\n  - url: a.exe\n" % v


class FeedMonotonic(unittest.TestCase):

    def test_ordering(self):
        self.assertTrue(fm.is_older("0.38.12", "0.38.13"))
        self.assertTrue(fm.is_older("0.9.0", "0.10.0"))
        self.assertFalse(fm.is_older("0.38.13", "0.38.12"))
        self.assertFalse(fm.is_older("0.38.13", "0.38.13"))
        self.assertTrue(fm.is_older("0.38.13-beta.2", "0.38.13"))
        self.assertTrue(fm.is_older("0.38.13-beta.2", "0.38.13-beta.10"))
        self.assertFalse(fm.is_older("0.38.14-beta.1", "0.38.13"))

    def test_version_parsing(self):
        self.assertEqual(fm.feed_version("version: '0.38.13'\npath: x\n"), "0.38.13")
        self.assertEqual(fm.feed_version(yml("0.38.13")), "0.38.13")

    def run_select(self, mine, published):
        with tempfile.TemporaryDirectory() as d:
            feeds, out = Path(d, "in"), Path(d, "out")
            feeds.mkdir()
            for name, v in mine.items():
                (feeds / name).write_text(yml(v))
            return fm.select(feeds, out, lambda n: yml(published[n]) if n in published else None)

    def test_late_older_run_is_skipped(self):
        kept = self.run_select({"latest.yml": "0.38.12", "latest-linux.yml": "0.38.12"},
                               {"latest.yml": "0.38.13", "latest-linux.yml": "0.38.13"})
        self.assertEqual(kept, [])

    def test_newer_equal_and_absent_are_published(self):
        kept = self.run_select({"latest.yml": "0.38.14", "latest-linux.yml": "0.38.13", "beta.yml": "0.38.14-beta.1"},
                               {"latest.yml": "0.38.13", "latest-linux.yml": "0.38.13"})
        self.assertEqual(kept, ["beta.yml", "latest-linux.yml", "latest.yml"])


if __name__ == "__main__":
    unittest.main()

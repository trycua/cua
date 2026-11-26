import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from npm_dist_tag import dist_tag  # noqa: E402


class DistTagTests(unittest.TestCase):
    def test_first_publish_is_latest(self):
        self.assertEqual(dist_tag("0.63.0", None), "latest")

    def test_newer_than_latest(self):
        self.assertEqual(dist_tag("0.63.1", "0.62.1"), "latest")

    def test_same_version_republish(self):
        self.assertEqual(dist_tag("0.63.1", "0.63.1"), "latest")

    def test_out_of_order_release_never_takes_latest(self):
        self.assertEqual(dist_tag("0.63.0", "0.63.1"), "backport-0.63")

    def test_prerelease_is_next(self):
        self.assertEqual(dist_tag("0.64.0-nightly.1", "0.63.1"), "next")

    def test_numeric_not_lexical_compare(self):
        self.assertEqual(dist_tag("0.10.0", "0.9.5"), "latest")

    def test_invalid(self):
        with self.assertRaises(ValueError):
            dist_tag("v1", None)


if __name__ == "__main__":
    unittest.main()

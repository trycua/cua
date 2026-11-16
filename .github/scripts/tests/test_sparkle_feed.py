"""Run: python3 .github/scripts/tests/test_sparkle_feed.py (CUA-3131)."""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import sparkle_feed as sf  # noqa: E402


def feed(*items):
    body = "".join(
        "<item><title>%s</title>%s<sparkle:version>%s</sparkle:version>"
        "<sparkle:shortVersionString>%s</sparkle:shortVersionString></item>"
        % (short, "<sparkle:channel>%s</sparkle:channel>" % ch if ch else "", build, short)
        for short, build, ch in items
    )
    return ('<rss xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle" version="2.0">'
            "<channel><title>Cua Spaces</title>%s</channel></rss>" % body)


class SparkleFeed(unittest.TestCase):

    def test_nightly_appcast_holds_only_nightlies(self):
        ok = feed(("0.21.0-nightly.20261105+55e1f5d", "0.20.0.202611050731", "nightly"))
        self.assertEqual(sf.nightly_violations(ok), [])
        self.assertIsNone(sf.stable_version(ok))
        stray = feed(("0.21.0-nightly.20261105+55e1f5d", "0.20.0.202611050731", "nightly"), ("0.20.0", "0.20.0.130", None))
        self.assertEqual(sf.nightly_violations(stray), ["0.20.0 is not a nightly"])
        untagged = feed(("0.21.0-nightly.20261105+55e1f5d", "0.20.0.202611050731", None))
        self.assertTrue(sf.nightly_violations(untagged))

    def test_beta_on_top_is_not_stable(self):
        # The 3 Nov 2026 feed: 0.20.0-beta.1 (beta) is the first item.
        f = feed(("0.20.0-beta.1", "0.20.0.129", "beta"), ("0.19.4", "0.19.4.128", None), ("0.19.3", "0.19.3.127", None))
        self.assertEqual(sf.stable_version(f), "0.19.4")
        self.assertEqual(sf.violations(f), [])

    def test_untagged_prerelease_fails(self):
        f = feed(("0.20.0-beta.1", "0.20.0.129", None), ("0.19.4", "0.19.4.128", None))
        self.assertEqual(len(sf.violations(f)), 1)
        self.assertEqual(sf.stable_version(f), "0.20.0-beta.1")  # what a Stable Mac would get

    def test_wrong_channels_fail(self):
        f = feed(("0.21.0-nightly.3", "0.21.0.140", "beta"), ("0.19.4", "0.19.4.128", "beta"))
        self.assertEqual(len(sf.violations(f)), 2)

    def test_nightly_channel(self):
        f = feed(("0.21.0-nightly.3", "0.21.0.140", "nightly"), ("0.19.4", "0.19.4.128", None))
        self.assertEqual(sf.violations(f), [])
        self.assertEqual(sf.stable_version(f), "0.19.4")

    def test_newest_by_build_not_order(self):
        f = feed(("0.19.3", "0.19.3.127", None), ("0.19.4", "0.19.4.128", None))
        self.assertEqual(sf.stable_version(f), "0.19.4")


if __name__ == "__main__":
    unittest.main()

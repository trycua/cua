# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
"""Run: python3 .github/scripts/tests/test_release_rollout.py"""

import contextlib
import io
import sys
import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import release_rollout as rr  # noqa: E402

NOW = datetime(2026, 10, 23, 12, 0, tzinfo=timezone.utc)
YML = "version: 0.13.1\nfiles:\n  - url: https://x/a.exe\npath: https://x/a.exe\nreleaseDate: '2026-10-22T00:00:00Z'\n"
APPCAST = """<?xml version='1.0' encoding='utf-8'?>
<rss xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle" version="2.0">
    <channel>
        <title>Cua Spaces</title>
        <item><title>0.13.1</title><pubDate>Thu, 22 Oct 2026 15:12:41 +0000</pubDate><sparkle:version>0.13.1.96</sparkle:version></item>
        <item><title>0.13.0</title><pubDate>Thu, 22 Oct 2026 05:36:29 +0000</pubDate><sparkle:version>0.13.0.95</sparkle:version></item>
        <item><title>0.14.0-beta.1</title><sparkle:channel>beta</sparkle:channel><sparkle:version>0.14.0.97</sparkle:version></item>
    </channel>
</rss>
"""


def run(files, stage, platform="all", channel="stable"):
    with tempfile.TemporaryDirectory() as d:
        feed, out = Path(d, "in"), Path(d, "out")
        feed.mkdir()
        for name, text in files.items():
            (feed / name).write_text(text)
        with contextlib.redirect_stdout(io.StringIO()):
            code = rr.main(["--feed-dir", str(feed), "--out-dir", str(out), "--platform", platform,
                            "--stage", stage, "--channel", channel, "--now", NOW.isoformat()])
        return code, ({p.name: p.read_text() for p in out.iterdir()} if out.exists() else {})


class Electron(unittest.TestCase):
    def test_stages(self):
        for stage, want in (("10", "stagingPercentage: 10"), ("50", "stagingPercentage: 50"), ("pause", "stagingPercentage: 0")):
            _, out = run({"latest.yml": YML}, stage, "windows")
            self.assertTrue(out["latest.yml"].endswith(want + "\n"))

    def test_100_removes_the_field_and_replaces_not_appends(self):
        _, out = run({"latest.yml": YML + "stagingPercentage: 10\n"}, "100", "windows")
        self.assertNotIn("stagingPercentage", out["latest.yml"])
        _, out = run({"latest.yml": YML + "stagingPercentage: 10\n"}, "50", "windows")
        self.assertEqual(out["latest.yml"].count("stagingPercentage"), 1)

    def test_stable_refuses_a_prerelease_feed(self):
        code, _ = run({"latest.yml": YML.replace("0.13.1", "0.14.0-beta.1")}, "10", "windows")
        self.assertEqual(code, 1)

    def test_beta_touches_only_beta_files(self):
        _, out = run({"latest.yml": YML, "beta.yml": YML.replace("0.13.1", "0.14.0-beta.1")}, "10", "windows", "beta")
        self.assertIn("beta.yml", out)
        self.assertNotIn("latest.yml", out)

    def test_platform_selects_files(self):
        _, out = run({"latest.yml": YML, "latest-linux.yml": YML, "latest-linux-arm64.yml": YML}, "10", "linux")
        self.assertEqual(sorted(k for k in out if k.endswith(".yml")), ["latest-linux-arm64.yml", "latest-linux.yml"])


class Sparkle(unittest.TestCase):
    def test_10_holds_group_zero_on_the_newest_stable_item_only(self):
        _, out = run({rr.APPCAST: APPCAST}, "10", "macos")
        xml = out[rr.APPCAST]
        self.assertEqual(xml.count("phasedRolloutInterval>"), 2)  # one open, one close tag
        self.assertIn(f"<sparkle:phasedRolloutInterval>{rr.HOLD_SECONDS}<", xml)
        self.assertIn("Fri, 23 Oct 2026 12:00:00 +0000", xml)

    def test_50_opens_four_groups(self):
        _, out = run({rr.APPCAST: APPCAST}, "50", "macos")
        self.assertIn(f">{rr.WEEK}<", out[rr.APPCAST])
        self.assertIn("Fri, 02 Oct 2026 12:00:00 +0000", out[rr.APPCAST])  # now - 3 weeks

    def test_100_removes_the_interval(self):
        _, staged = run({rr.APPCAST: APPCAST}, "10", "macos")
        _, out = run({rr.APPCAST: staged[rr.APPCAST]}, "100", "macos")
        self.assertNotIn("phasedRolloutInterval", out[rr.APPCAST])

    def test_pause_removes_item_and_resume_restores_it(self):
        _, out = run({rr.APPCAST: APPCAST}, "pause", "macos")
        self.assertNotIn("<title>0.13.1</title>", out[rr.APPCAST])
        self.assertIn("0.13.0", out[rr.APPCAST])
        self.assertIn("0.14.0-beta.1", out[rr.APPCAST])  # beta untouched
        self.assertIn("<title>0.13.1</title>", out[rr.PAUSED])
        _, back = run({rr.APPCAST: out[rr.APPCAST], rr.PAUSED: out[rr.PAUSED]}, "10", "macos")
        self.assertIn("<title>0.13.1</title>", back[rr.APPCAST])
        self.assertIn(rr.PAUSED, back["deletions.txt"])

    def test_beta_rehearsal_picks_the_beta_item(self):
        _, out = run({rr.APPCAST: APPCAST}, "10", "macos", "beta")
        xml = out[rr.APPCAST]
        self.assertEqual(xml.count("phasedRolloutInterval>"), 2)
        beta = xml.split("0.14.0-beta.1")[1]
        self.assertIn("phasedRolloutInterval", beta)


if __name__ == "__main__":
    unittest.main()

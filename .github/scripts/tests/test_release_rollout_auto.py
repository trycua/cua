"""Run: python3 .github/scripts/tests/test_release_rollout_auto.py"""

import argparse
import sys
import tempfile
import unittest
from datetime import datetime, timedelta, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import release_rollout_auto as ra  # noqa: E402

PUB = datetime(2026, 10, 25, 21, 47, 23, tzinfo=timezone.utc)
SP = 'xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle"'


def feeds(staging, interval, pub=PUB, title="0.16.0"):
    d = Path(tempfile.mkdtemp())
    yml = "version: %s\nreleaseDate: '2026-10-25T21:29:51Z'\n" % title
    if staging is not None:
        yml += "stagingPercentage: %s\n" % staging
    (d / "latest.yml").write_text(yml)
    iv = "<sparkle:phasedRolloutInterval>%s</sparkle:phasedRolloutInterval>" % interval if interval else ""
    (d / "cua-spaces-appcast.xml").write_text(
        "<rss %s version='2.0'><channel><item><title>%s</title><pubDate>%s</pubDate>%s<sparkle:version>%s.1</sparkle:version></item></channel></rss>"
        % (SP, title, pub.strftime("%a, %d %b %Y %H:%M:%S +0000"), iv, title))
    return d


def args(**kw):
    a = argparse.Namespace(min_age_hours=24, min_installs=15, min_create_events=30, max_create_error_delta=10.0,
                           min_update_events=20, max_update_failure_delta=15.0,
                           min_sessions=100, max_crash_free_drop=0.5, min_crash_free=99.0)
    a.__dict__.update(kw)
    return a


def fake(installs=40, new_create=(50, 5), old_create=(100, 10), new_update=(100, 10), old_update=(100, 10), prev="0.15.0"):
    """A PostHog stand-in keyed on the query text. *_create = (total, errors); *_update = (attempts, failed)."""
    def q(sql):
        if "count(distinct distinct_id) n" in sql:
            return [[prev, 90]]
        if "count(distinct distinct_id) from" in sql:
            return [[installs]]
        if "cua_space_create" in sql:
            t, e = new_create if "'0.16.0'" in sql else old_create
            return [["ok", t - e], ["error", e]]
        if "cua_app_update" in sql:
            lo = sql.split("timestamp >= toDateTime('")[1][:10]
            t, f = new_update if lo >= "2026-10-25" else old_update
            return [["found", t - f], ["failed", f]]
        raise AssertionError(sql)
    return q


CR = lambda: {"gate": "crash", "status": "pass", "detail": "ok"}  # noqa: E731
OK = lambda: ra.gate_incident("k", lambda: {"incidents": []})  # noqa: E731


class State(unittest.TestCase):
    def test_stage_10(self):
        s = ra.read_state(feeds("10", ra.rr.HOLD_SECONDS))
        self.assertEqual((s["stage"], s["since"]), ("10", PUB))

    def test_stage_50_started_three_weeks_after_pubdate(self):
        s = ra.read_state(feeds("50", ra.rr.WEEK, PUB - timedelta(weeks=3)))
        self.assertEqual((s["stage"], s["since"]), ("50", PUB))

    def test_100_pause_and_disagreement(self):
        self.assertEqual(ra.read_state(feeds(None, None))["stage"], "100")
        self.assertEqual(ra.read_state(feeds("0", None))["stage"], "pause")
        self.assertTrue(ra.read_state(feeds("50", ra.rr.HOLD_SECONDS))["problem"])
        self.assertTrue(ra.read_state(feeds("10", ra.rr.HOLD_SECONDS, title="0.16.1"))["problem"] is None)


def sessions(**by_version):
    """A Sentry stand-in: version -> (sessions, crash-free fraction)."""
    return lambda v: by_version[v]


NOW = PUB + timedelta(hours=25)
CFG = {"org": "trycua", "project": "cua-spaces-desktop"}


def crash(new, prev, token="t", **kw):
    return ra.gate_crash(token, "0.16.0", "0.15.0", PUB, NOW, args(**kw), cfg=CFG, fetch=sessions(**{"0.16.0": new, "0.15.0": prev}))


class CrashGate(unittest.TestCase):
    def test_within_half_a_point_passes(self):
        self.assertEqual(crash((500, 0.992), (5000, 0.996))["status"], "pass")

    def test_more_than_half_a_point_below_fails(self):
        g = crash((500, 0.985), (5000, 0.996))
        self.assertEqual(g["status"], "fail")
        self.assertIn("drop +1.10", g["detail"])

    def test_better_than_before_passes(self):
        self.assertEqual(crash((500, 0.999), (5000, 0.99))["status"], "pass")

    def test_too_few_sessions_waits(self):
        self.assertEqual(crash((99, 0.5), (5000, 0.996))["status"], "wait")

    def test_no_baseline_uses_the_floor(self):
        self.assertEqual(crash((500, 0.995), (0, None))["status"], "pass")
        self.assertEqual(crash((500, 0.97), (3, 1.0))["status"], "fail")

    def test_no_token_skips_and_errors_wait(self):
        self.assertEqual(crash((500, 1.0), (500, 1.0), token="")["status"], "skip")

        def boom(v):
            raise OSError("down")
        g = ra.gate_crash("t", "0.16.0", "0.15.0", PUB, NOW, args(), cfg=CFG, fetch=boom)
        self.assertEqual(g["status"], "wait")


class Decide(unittest.TestCase):
    def go(self, hours=25, hold="", q=None, stage="10", crash=None):
        s = ra.read_state(feeds("10", ra.rr.HOLD_SECONDS)) if stage == "10" else ra.read_state(feeds(stage, None))
        return ra.decide(s, PUB + timedelta(hours=hours), args(), q=q or fake(), incident=OK(), hold=hold, crash=crash or CR())

    def test_advance(self):
        d = self.go()
        self.assertEqual((d["action"], d["next"]), ("advance", "50"))

    def test_too_young_waits_even_if_green(self):
        self.assertEqual(self.go(hours=5)["action"], "wait")

    def test_no_data_waits(self):
        self.assertEqual(self.go(q=fake(installs=3))["action"], "wait")

    def test_create_errors_pause(self):
        d = self.go(q=fake(new_create=(50, 25)))
        self.assertEqual(d["action"], "pause")

    def test_update_failures_pause_even_when_young(self):
        self.assertEqual(self.go(hours=2, q=fake(new_update=(100, 60)))["action"], "pause")

    def test_incident_pauses(self):
        s = ra.read_state(feeds("10", ra.rr.HOLD_SECONDS))
        inc = ra.gate_incident("k", lambda: {"incidents": [{"reference": "INC-9", "name": "Spaces relay down"}]})
        self.assertEqual(ra.decide(s, PUB + timedelta(hours=30), args(), q=fake(), incident=inc)["action"], "pause")

    def test_crash_gate_pauses(self):
        bad = {"gate": "crash", "status": "fail", "detail": "crash-free sessions dropped"}
        d = self.go(crash=bad)
        self.assertEqual(d["action"], "pause")

    def test_hold_blocks_advance_not_pause(self):
        self.assertEqual(self.go(hold="label")["action"], "hold")
        self.assertEqual(self.go(hold="label", q=fake(new_create=(50, 25)))["action"], "pause")

    def test_done_and_paused_do_nothing(self):
        self.assertEqual(self.go(stage=None)["action"], "none")
        self.assertEqual(self.go(stage="0")["action"], "none")

    def test_incident_skipped_without_key(self):
        self.assertEqual(ra.gate_incident(None)["status"], "skip")


if __name__ == "__main__":
    unittest.main()

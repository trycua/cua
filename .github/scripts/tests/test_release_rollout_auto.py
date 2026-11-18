"""Run: python3 .github/scripts/tests/test_release_rollout_auto.py"""

import argparse
import os
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
    a = argparse.Namespace(soak_hours_10=2, soak_hours_50=4, soak_hours_full=24, min_installs=15, min_create_events=30, max_create_error_delta=10.0,
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
CFG = {"org": "cua-ai", "project": "cua-spaces-desktop"}


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


class SentryHostAndFailures(unittest.TestCase):
    def test_host_defaults_to_the_eu_region_and_is_configurable(self):
        os.environ.pop("SENTRY_API_HOST", None)
        self.assertEqual(ra.sentry_base(), "https://de.sentry.io/api/0")
        os.environ["SENTRY_API_HOST"] = "https://example.invalid/"
        try:
            self.assertEqual(ra.sentry_base(), "https://example.invalid/api/0")
        finally:
            os.environ.pop("SENTRY_API_HOST")

    def test_unreadable_sentry_waits_with_a_clear_message(self):
        import urllib.error
        denied = urllib.error.HTTPError("https://de.sentry.io/api/0/projects/cua-ai/x/", 403, "Forbidden", {}, None)

        def deny(v):
            raise denied
        g = ra.gate_crash("t", "0.16.0", "0.15.0", PUB, NOW, args(), cfg=CFG, fetch=deny)
        self.assertEqual(g["status"], "wait")
        self.assertIn("HTTP 403 from https://de.sentry.io", g["detail"])
        self.assertIn("not advancing", g["detail"])
        d = Decide().go(crash=g)
        self.assertEqual(d["action"], "wait")
        self.assertNotEqual(d["action"], "advance")


class Decide(unittest.TestCase):
    def go(self, hours=25, hold="", q=None, stage="10", crash=None, title="0.16.0"):
        s = ra.read_state(feeds("10", ra.rr.HOLD_SECONDS, title=title)) if stage == "10" else ra.read_state(feeds(stage, None, title=title))
        return ra.decide(s, PUB + timedelta(hours=hours), args(), q=q or fake(), incident=OK(), hold=hold, crash=crash or CR())

    def test_patch_advances_after_the_short_soak_at_10(self):
        self.assertEqual(self.go(hours=1.5, title="0.16.1")["action"], "wait")
        d = self.go(hours=2.5, title="0.16.1")
        self.assertEqual((d["action"], d["next"], d["soak_hours"]), ("advance", "50", 2))

    def test_patch_advances_after_the_short_soak_at_50(self):
        s = ra.read_state(feeds("50", ra.rr.WEEK, title="0.16.1"))
        three_weeks = timedelta(weeks=3)
        self.assertEqual(ra.decide(s, PUB + three_weeks + timedelta(hours=3), args(), q=fake(), incident=OK(), crash=CR())["action"], "wait")
        d = ra.decide(s, PUB + three_weeks + timedelta(hours=4.5), args(), q=fake(), incident=OK(), crash=CR())
        self.assertEqual((d["action"], d["next"], d["soak_hours"]), ("advance", "100", 4))

    def test_minor_and_major_keep_24h_at_every_step(self):
        for title in ("0.16.0", "1.0.0"):
            self.assertEqual(self.go(hours=5, title=title)["action"], "wait")
            self.assertEqual(self.go(hours=25, title=title)["soak_hours"], 24)
            self.assertEqual(self.go(hours=25, title=title)["action"], "advance")

    def test_short_soak_needs_enough_crash_sessions(self):
        few = {"gate": "crash", "status": "wait", "detail": "30 sessions (need 100)"}
        d = self.go(hours=5, title="0.16.1", crash=few)
        self.assertEqual(d["action"], "wait")
        skipped = {"gate": "crash", "status": "skip", "detail": "no token"}
        d = self.go(hours=5, title="0.16.1", crash=skipped)
        self.assertEqual((d["action"], d["soak_hours"]), ("wait", 24))
        self.assertEqual(self.go(hours=25, title="0.16.1", crash=skipped)["action"], "advance")

    def test_patch_still_waits_for_data_after_the_short_soak(self):
        self.assertEqual(self.go(hours=5, title="0.16.1", q=fake(installs=3))["action"], "wait")

    def test_is_minor_or_major(self):
        for v, want in (("0.16.0", True), ("1.0.0", True), ("0.16.1", False), ("2.3.10", False), ("0.16.0-beta.1", True), (None, False), ("junk", False)):
            self.assertEqual(ra.is_minor_or_major(v), want, v)

    def test_config_names_every_threshold(self):
        cfg = ra.load_soak()
        self.assertEqual((cfg["soak_hours_10"], cfg["soak_hours_50"], cfg["soak_hours_full"]), (2, 4, 24))
        self.assertTrue(set(args().__dict__) <= set(cfg))

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

"""Readiness and failure diagnostics, not native compositor evidence."""

import unittest
from unittest.mock import patch

import live_discovery as live


class DiscoveryReadinessTest(unittest.TestCase):
    def test_requires_consecutive_equal_samples(self):
        first = {"cursor": {"x": 0, "y": 0}}
        settled = {"cursor": {"x": 100, "y": 100}}
        with patch.object(live, "desktop_state", side_effect=[first, first, settled, settled, settled]) as observe, \
                patch.object(live.time, "sleep"):
            self.assertEqual(live.stable_desktop_state(), settled)
        self.assertEqual(observe.call_count, 5)

    def test_unstable_preflight_fails(self):
        with patch.object(live, "desktop_state", side_effect=[{"cursor": 1}, {"cursor": 2}]), \
                patch.object(live.time, "monotonic", side_effect=[0, 0, 1, 6]), \
                patch.object(live.time, "sleep"):
            with self.assertRaisesRegex(TimeoutError, "before discovery requests"):
                live.stable_desktop_state()

    def test_observation_failure_is_not_swallowed(self):
        with patch.object(live, "desktop_state", side_effect=RuntimeError("query failed")):
            with self.assertRaisesRegex(RuntimeError, "query failed"):
                live.stable_desktop_state()

    def test_post_request_change_still_fails_with_sanitized_delta(self):
        before = {"active_window": "private-address", "active_workspace": 1,
                  "cursor": {"x": 10, "y": 20}, "windows": [("private-window",)]}
        after = {**before, "cursor": {"x": 11, "y": 20}}
        with self.assertRaises(AssertionError) as result:
            live.assert_unchanged(before, after)
        message = str(result.exception)
        self.assertIn('"changed_fields": ["cursor"]', message)
        self.assertIn('"x": 11', message)
        self.assertNotIn("private-address", message)
        self.assertNotIn("private-window", message)
        live.assert_unchanged(before, before)


if __name__ == "__main__":
    unittest.main()

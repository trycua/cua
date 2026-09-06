"""Protocol, readiness, and diagnostic checks; not native compositor evidence."""

import os
import subprocess
import sys
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import live_discovery as discovery


class DiscoveryReadinessTest(unittest.TestCase):
    def test_requires_consecutive_equal_samples(self):
        first = {"cursor": {"x": 0, "y": 0}}
        settled = {"cursor": {"x": 100, "y": 100}}
        with patch.object(discovery, "desktop_state", side_effect=[first, first, settled, settled, settled]) as observe, \
                patch.object(discovery.time, "sleep"):
            self.assertEqual(discovery.stable_desktop_state(), settled)
        self.assertEqual(observe.call_count, 5)

    def test_unstable_preflight_fails(self):
        with patch.object(discovery, "desktop_state", side_effect=[{"cursor": 1}, {"cursor": 2}]), \
                patch.object(discovery.time, "monotonic", side_effect=[0, 0, 1, 6]), \
                patch.object(discovery.time, "sleep"):
            with self.assertRaisesRegex(TimeoutError, "before discovery requests"):
                discovery.stable_desktop_state()

    def test_observation_failure_is_not_swallowed(self):
        with patch.object(discovery, "desktop_state", side_effect=RuntimeError("query failed")):
            with self.assertRaisesRegex(RuntimeError, "query failed"):
                discovery.stable_desktop_state()

    def test_post_request_change_still_fails_with_sanitized_delta(self):
        before = {"active_window": "private-address", "active_workspace": 1,
                  "cursor": {"x": 10, "y": 20}, "windows": [("private-window",)]}
        after = {**before, "cursor": {"x": 11, "y": 20}}
        with self.assertRaises(AssertionError) as result:
            discovery.assert_unchanged(before, after)
        message = str(result.exception)
        self.assertIn('"changed_fields": ["cursor"]', message)
        self.assertIn('"x": 11', message)
        self.assertNotIn("private-address", message)
        self.assertNotIn("private-window", message)
        discovery.assert_unchanged(before, before)


class DiscoveryTest(unittest.TestCase):
    def test_exchange_sends_and_correlates_request(self):
        packet = discovery.HEADER.pack(b"CUA2", 2, 0, 3, 0, 17, 0)
        client = Mock()
        client.send.return_value = len(packet)
        client.recv.return_value = discovery.HEADER.pack(b"CUA2", 2, 0, 4, 0, 17, 0)
        self.assertEqual(discovery.exchange(client, 3, 17), (4, b""))
        client.send.assert_called_once_with(packet)

    def test_exchange_rejects_short_send_before_receive(self):
        client = Mock()
        client.send.return_value = 0
        with self.assertRaises(AssertionError):
            discovery.exchange(client, 3, 17)
        client.recv.assert_not_called()

    def test_exchange_rejects_wrong_request_id(self):
        client = Mock()
        client.send.return_value = discovery.HEADER.size
        client.recv.return_value = discovery.HEADER.pack(b"CUA2", 2, 0, 4, 0, 18, 0)
        with self.assertRaises(AssertionError):
            discovery.exchange(client, 3, 17)

    def test_optimized_python_refuses_before_cli_or_compositor_access(self):
        script = str(Path(discovery.__file__).resolve())
        for options, optimize in ((["-O"], ""), ([], "1"), (["-OO"], "")):
            with self.subTest(options=options, optimize=optimize):
                result = subprocess.run(
                    [sys.executable, *options, script, "--help"],
                    env={**os.environ, "PYTHONOPTIMIZE": optimize},
                    capture_output=True,
                    text=True,
                    timeout=10,
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("Discovery validation requires assertions", result.stderr)
                self.assertNotIn("usage:", result.stdout)


if __name__ == "__main__":
    unittest.main()

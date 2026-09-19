import importlib.util
from pathlib import Path
import unittest
from unittest import mock


MODULE_PATH = Path(__file__).with_name("main.py")
SPEC = importlib.util.spec_from_file_location("visual_only_canvas", MODULE_PATH)
fixture = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(fixture)


class VisualOnlyCanvasTests(unittest.TestCase):
    def test_hit_testing_uses_painted_geometry(self):
        self.assertEqual(fixture.card_at(174, 220), "save")
        self.assertEqual(fixture.card_at(394, 220), "send")
        self.assertEqual(fixture.card_at(614, 220), "cancel")
        self.assertIsNone(fixture.card_at(40, 220))

    def test_oracle_contains_behavior_without_coordinates_or_labels(self):
        self.assertEqual(
            fixture.oracle_state("send", 3),
            {
                "fixture": "visual-only-canvas/v1",
                "ready": True,
                "selected": "send",
                "action_count": 3,
            },
        )

    def test_oracle_transport_is_loopback_only(self):
        fixture.validate_journal_url("http://127.0.0.1:4321/state")
        with self.assertRaisesRegex(ValueError, "loopback"):
            fixture.validate_journal_url("https://example.test/state")

    @mock.patch.object(fixture.subprocess, "run")
    def test_xprop_is_bounded_and_captures_diagnostics(self, run):
        fixture._xprop("-root", "_NET_CLIENT_LIST")

        run.assert_called_once_with(
            ["xprop", "-root", "_NET_CLIENT_LIST"],
            check=True,
            capture_output=True,
            text=True,
            timeout=1,
        )

    @mock.patch.object(fixture.sys, "platform", "linux")
    @mock.patch.object(fixture.os, "getpid", return_value=4242)
    @mock.patch.object(fixture, "_xprop")
    def test_linux_fixture_discovers_client_by_exact_title_and_verifies_owner(self, xprop, _getpid):
        xprop.side_effect = [
            mock.Mock(stdout="_NET_CLIENT_LIST(WINDOW): window id # 0x100001, 0x20001e\n"),
            mock.Mock(stdout='_NET_WM_NAME(UTF8_STRING) = "Other window"\n'),
            mock.Mock(stdout='_NET_WM_NAME(UTF8_STRING) = "Unique fixture title"\n'),
            mock.Mock(stdout=""),
            mock.Mock(stdout="_NET_WM_PID(CARDINAL) = 4242\n"),
        ]

        fixture.publish_x11_owner("Unique fixture title", attempts=1, interval_seconds=0)

        self.assertEqual(
            xprop.call_args_list,
            [
                mock.call("-root", "_NET_CLIENT_LIST_STACKING", "_NET_CLIENT_LIST"),
                mock.call("-id", "0x100001", "_NET_WM_NAME", "WM_NAME"),
                mock.call("-id", "0x20001e", "_NET_WM_NAME", "WM_NAME"),
                mock.call(
                    "-id",
                    "0x20001e",
                    "-f",
                    "_NET_WM_PID",
                    "32c",
                    "-set",
                    "_NET_WM_PID",
                    "4242",
                ),
                mock.call("-id", "0x20001e", "_NET_WM_PID"),
            ],
        )

    @mock.patch.object(fixture.sys, "platform", "linux")
    @mock.patch.object(fixture.time, "sleep")
    @mock.patch.object(fixture, "_xprop")
    def test_linux_fixture_retries_then_reports_observed_titles(self, xprop, sleep):
        xprop.side_effect = [
            mock.Mock(stdout="_NET_CLIENT_LIST(WINDOW): window id # 0x100001\n"),
            mock.Mock(stdout='WM_NAME(STRING) = "Other window"\n'),
            mock.Mock(stdout="_NET_CLIENT_LIST(WINDOW): window id # 0x100001\n"),
            mock.Mock(stdout='WM_NAME(STRING) = "Still other"\n'),
        ]

        with self.assertRaisesRegex(RuntimeError, r"attempt 2/2.*0x100001='Still other'"):
            fixture.publish_x11_owner("Missing title", attempts=2, interval_seconds=0.01)

        sleep.assert_called_once_with(0.01)

    @mock.patch.object(fixture.sys, "platform", "linux")
    @mock.patch.object(fixture, "_xprop")
    def test_linux_fixture_rejects_duplicate_exact_titles(self, xprop):
        xprop.side_effect = [
            mock.Mock(stdout="_NET_CLIENT_LIST(WINDOW): window id # 0x100001, 0x20001e\n"),
            mock.Mock(stdout='_NET_WM_NAME(UTF8_STRING) = "Duplicate title"\n'),
            mock.Mock(stdout='_NET_WM_NAME(UTF8_STRING) = "Duplicate title"\n'),
        ]

        with self.assertRaisesRegex(RuntimeError, "matched multiple EWMH clients"):
            fixture.publish_x11_owner("Duplicate title", attempts=1, interval_seconds=0)

    @mock.patch.object(fixture.sys, "platform", "linux")
    @mock.patch.object(fixture.os, "getpid", return_value=4242)
    @mock.patch.object(fixture, "_xprop")
    def test_linux_fixture_rejects_unverified_owner(self, xprop, _getpid):
        xprop.side_effect = [
            mock.Mock(stdout="_NET_CLIENT_LIST(WINDOW): window id # 0x20001e\n"),
            mock.Mock(stdout='_NET_WM_NAME(UTF8_STRING) = "Unique fixture title"\n'),
            mock.Mock(stdout=""),
            mock.Mock(stdout="_NET_WM_PID(CARDINAL) = 9999\n"),
        ]

        with self.assertRaisesRegex(RuntimeError, "reported _NET_WM_PID=9999; expected 4242"):
            fixture.publish_x11_owner("Unique fixture title", attempts=1, interval_seconds=0)


if __name__ == "__main__":
    unittest.main()

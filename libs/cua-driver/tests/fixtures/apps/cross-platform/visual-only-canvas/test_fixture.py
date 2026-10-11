import contextlib
import importlib.util
import io
from pathlib import Path
import unittest
from unittest import mock


MODULE_PATH = Path(__file__).with_name("main.py")
SPEC = importlib.util.spec_from_file_location("visual_only_canvas", MODULE_PATH)
fixture = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(fixture)


class VisualOnlyCanvasTests(unittest.TestCase):
    def test_send_label_font_pixels_are_bounded(self):
        self.assertEqual(fixture.send_label_font_pixels("1"), 1)
        self.assertEqual(fixture.send_label_font_pixels("48"), 48)
        for value in ("0", "49", "not-an-integer"):
            with self.subTest(value=value), self.assertRaises(
                fixture.argparse.ArgumentTypeError
            ):
                fixture.send_label_font_pixels(value)

    def test_send_label_font_override_does_not_change_other_cards(self):
        self.assertEqual(fixture.card_label_font_pixels("send", 48), 48)
        self.assertEqual(
            fixture.card_label_font_pixels("save", 48), fixture.CARD_LABEL_FONT_PIXELS
        )
        self.assertEqual(
            fixture.card_label_font_pixels("cancel", 48), fixture.CARD_LABEL_FONT_PIXELS
        )

    def test_hit_testing_uses_painted_geometry(self):
        self.assertEqual(fixture.card_at(174, 220), "save")
        self.assertEqual(fixture.card_at(394, 220), "send")
        self.assertEqual(fixture.card_at(614, 220), "cancel")
        self.assertIsNone(fixture.card_at(40, 220))

    def test_background_click_is_bound_on_canvas_and_toplevel(self):
        toplevel = mock.Mock()
        canvas = mock.Mock()
        handler = mock.Mock()

        fixture.bind_background_click(toplevel, canvas, handler)

        canvas.bind.assert_called_once_with("<Button-1>", handler)
        toplevel.bind.assert_called_once_with("<Button-1>", handler)

    def test_toplevel_event_coordinates_are_translated_to_the_canvas(self):
        canvas = mock.Mock()
        canvas.winfo_rootx.return_value = 120
        canvas.winfo_rooty.return_value = 80
        event = mock.Mock(x_root=514, y_root=300)

        self.assertEqual(fixture.canvas_point(event, canvas), (394, 220))

    def test_toplevel_click_updates_the_oracle_once(self):
        visual_fixture = fixture.VisualFixture.__new__(fixture.VisualFixture)
        visual_fixture.canvas = mock.Mock()
        visual_fixture.canvas.winfo_rootx.return_value = 120
        visual_fixture.canvas.winfo_rooty.return_value = 80
        visual_fixture.selected = None
        visual_fixture.action_count = 0
        visual_fixture.paint = mock.Mock()
        visual_fixture.publish = mock.Mock()

        result = visual_fixture.on_click(mock.Mock(x_root=514, y_root=300))

        self.assertEqual(visual_fixture.selected, "send")
        self.assertEqual(visual_fixture.action_count, 1)
        self.assertEqual(result, "break")
        visual_fixture.paint.assert_called_once_with()
        visual_fixture.publish.assert_called_once_with()

    def test_click_outside_cards_stops_bindtag_propagation_without_mutating_oracle(self):
        visual_fixture = fixture.VisualFixture.__new__(fixture.VisualFixture)
        visual_fixture.canvas = mock.Mock()
        visual_fixture.canvas.winfo_rootx.return_value = 120
        visual_fixture.canvas.winfo_rooty.return_value = 80
        visual_fixture.selected = None
        visual_fixture.action_count = 0
        visual_fixture.paint = mock.Mock()
        visual_fixture.publish = mock.Mock()

        diagnostic = io.StringIO()
        with contextlib.redirect_stderr(diagnostic):
            result = visual_fixture.on_click(mock.Mock(x_root=130, y_root=90))

        self.assertEqual(result, "break")
        self.assertIn("ignored click outside cards at canvas (10, 10)", diagnostic.getvalue())
        self.assertIsNone(visual_fixture.selected)
        self.assertEqual(visual_fixture.action_count, 0)
        visual_fixture.paint.assert_not_called()
        visual_fixture.publish.assert_not_called()

    @mock.patch.object(fixture.os, "getpid", return_value=4242)
    def test_oracle_contains_behavior_and_process_identity_without_coordinates_or_labels(
        self, _getpid
    ):
        self.assertEqual(
            fixture.oracle_state("send", 3),
            {
                "fixture": "visual-only-canvas/v1",
                "pid": 4242,
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

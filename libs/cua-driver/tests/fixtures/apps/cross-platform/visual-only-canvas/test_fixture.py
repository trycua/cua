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

    @mock.patch.object(fixture.sys, "platform", "linux")
    @mock.patch.object(fixture.subprocess, "run")
    def test_linux_fixture_publishes_verified_x11_owner(self, run):
        fixture.publish_x11_owner(0x20001E, 4242)

        run.assert_called_once_with(
            [
                "xprop",
                "-id",
                str(0x20001E),
                "-f",
                "_NET_WM_PID",
                "32c",
                "-set",
                "_NET_WM_PID",
                "4242",
            ],
            check=True,
            stdout=fixture.subprocess.DEVNULL,
            stderr=fixture.subprocess.PIPE,
            text=True,
        )


if __name__ == "__main__":
    unittest.main()

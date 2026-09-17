import importlib.util
from pathlib import Path
import unittest


MODULE_PATH = Path(__file__).with_name("main.py")
SPEC = importlib.util.spec_from_file_location("visual_only_canvas", MODULE_PATH)
fixture = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(fixture)


class VisualOnlyCanvasTests(unittest.TestCase):
    def test_hit_testing_uses_painted_geometry(self):
        self.assertEqual(fixture.card_at(174, 220), "ember")
        self.assertEqual(fixture.card_at(394, 220), "tide")
        self.assertEqual(fixture.card_at(614, 220), "moss")
        self.assertIsNone(fixture.card_at(40, 220))

    def test_oracle_contains_behavior_without_coordinates_or_labels(self):
        self.assertEqual(
            fixture.oracle_state("moss", 3),
            {
                "fixture": "visual-only-canvas/v1",
                "ready": True,
                "selected": "moss",
                "action_count": 3,
            },
        )

    def test_oracle_transport_is_loopback_only(self):
        fixture.validate_journal_url("http://127.0.0.1:4321/state")
        with self.assertRaisesRegex(ValueError, "loopback"):
            fixture.validate_journal_url("https://example.test/state")


if __name__ == "__main__":
    unittest.main()

import sys
import unittest
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "web-common"))
from http_helpers import WebTest


class SignupTest(WebTest, unittest.TestCase):
    bundle = BUNDLE
    variants = [
        {
            "name": "Mira Vale",
            "email": "mira@example.test",
            "password": "FernTrail42",
            "plan": "plus",
            "invite": "FERN-41",
        },
        {
            "name": "Oren Brook",
            "email": "oren@example.test",
            "password": "BrookPath73",
            "plan": "basic",
            "invite": "FERN-73",
        },
    ]

    def submit(self, **changes):
        return self.request("/signup", {**self.expected, **changes})

    def test_success_reset_and_seed(self):
        self.assertFalse(self.verdict()["passed"])
        before = self.request("/__state")
        self.assertEqual(before, self.request("/__state"))
        self.assertEqual(self.submit()[0], 200)
        self.assertTrue(self.verdict()["passed"])
        self.restart()
        self.assertEqual(self.request("/__state")[1]["accounts"], [])
        self.assertFalse(self.verdict()["passed"])
        original = self.expected
        self.restart(seed=1)
        self.assertNotEqual(original, self.expected)
        brief = (self.workspace / ".runtime/brief.md").read_text()
        for value in self.expected.values():
            self.assertIn(str(value), brief)
        self.assertEqual(self.request("/__state")[1]["variant_id"], 1)

    def test_errors_can_be_corrected_without_partial_records(self):
        status, body = self.submit(email="broken", invite="WRONG")
        self.assertEqual(status, 422)
        self.assertIn("email", body["errors"])
        self.assertIn("invite", body["errors"])
        self.assertEqual(self.request("/__state")[1]["accounts"], [])
        self.assertEqual(self.submit()[0], 200)
        self.assertTrue(self.verdict()["passed"])

    def test_wrong_accepted_value_and_duplicate(self):
        self.assertEqual(self.submit(name="Wrong Person")[0], 200)
        self.assertFalse(self.verdict()["passed"])
        self.restart()
        self.submit()
        self.submit()
        self.assertEqual(len(self.request("/__state")[1]["accounts"]), 2)
        self.assertFalse(self.verdict()["passed"])

    def test_manifest_and_isolated_evaluator(self):
        self.check_integrity_and_hidden_layout()


if __name__ == "__main__":
    unittest.main()

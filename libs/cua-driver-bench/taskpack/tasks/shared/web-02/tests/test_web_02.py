import sys
import unittest
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "web-common"))
from http_helpers import WebTest


class CheckoutTest(WebTest, unittest.TestCase):
    bundle = BUNDLE
    variants = [
        {
            "items": {"tea": 2, "cup": 3},
            "address": {
                "name": "Mira Vale",
                "street": "14 Fern Lane",
                "city": "Pebble Bay",
                "postal": "41002",
            },
            "card": "4111111111111111",
            "expiry": "12/29",
            "cvv": "123",
        },
        {
            "items": {"tea": 4, "cup": 1},
            "address": {
                "name": "Oren Brook",
                "street": "27 Moss Road",
                "city": "Willow Bend",
                "postal": "73004",
            },
            "card": "5555555555554444",
            "expiry": "11/30",
            "cvv": "456",
        },
    ]

    def checkout(self, **changes):
        data = {**self.expected, **changes}
        self.assertEqual(self.request("/cart", {"items": data["items"]})[0], 200)
        self.assertEqual(self.request("/address", data)[0], 200)
        return self.request("/confirm", {})

    def test_success_reset_and_seed(self):
        self.assertFalse(self.verdict()["passed"])
        self.assertEqual(self.request("/__state"), self.request("/__state"))
        self.assertEqual(self.checkout()[0], 200)
        self.assertTrue(self.verdict()["passed"])
        state = self.request("/__state")[1]
        self.assertNotIn("card", state["orders"][0])
        self.assertNotIn("cvv", state["orders"][0])
        original = self.expected
        self.restart(seed=1)
        self.assertEqual(self.request("/__state")[1]["orders"], [])
        self.assertFalse(self.verdict()["passed"])
        self.assertNotEqual(original, self.expected)
        brief = (self.workspace / ".runtime/brief.md").read_text()
        self.assertIn(self.expected["card"], brief)
        self.assertEqual(self.request("/__state")[1]["variant_id"], 1)

    def test_invalid_and_incomplete_then_correction(self):
        self.assertEqual(self.request("/confirm", {})[0], 422)
        self.assertEqual(self.request("/cart", {"items": {"tea": -1}})[0], 422)
        self.assertEqual(self.request("/cart", {"items": self.expected["items"]})[0], 200)
        self.assertFalse(self.verdict()["passed"])
        status, body = self.request("/address", {**self.expected, "card": "123"})
        self.assertEqual(status, 422)
        self.assertIn("card", body["errors"])
        self.assertEqual(self.request("/__state")[1]["orders"], [])
        self.assertEqual(self.checkout()[0], 200)
        self.assertTrue(self.verdict()["passed"])

    def test_wrong_accepted_value_and_duplicate(self):
        self.checkout(address={**self.expected["address"], "city": "Wrong City"})
        self.assertFalse(self.verdict()["passed"])
        self.restart()
        self.checkout()
        self.assertEqual(self.request("/confirm", {})[0], 200)
        self.assertEqual(len(self.request("/__state")[1]["orders"]), 2)
        self.assertFalse(self.verdict()["passed"])

    def test_manifest_and_isolated_evaluator(self):
        self.check_integrity_and_hidden_layout()


if __name__ == "__main__":
    unittest.main()

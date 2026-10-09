"""Independent negative tests for the pytest-json-report receipt verifier."""
import importlib.util
from pathlib import Path
import unittest

source = Path(__file__).resolve().parents[2] / "scripts" / "verify_pytest_receipt.py"
spec = importlib.util.spec_from_file_location("verify_pytest_receipt", source)
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)


class ReceiptTests(unittest.TestCase):
    def valid(self):
        return {
            "exitcode": 0,
            "summary": {"collected": 2, "total": 2, "passed": 2},
            "tests": [
                {"nodeid": "a.py::test_one", "outcome": "passed"},
                {"nodeid": "a.py::test_two", "outcome": "passed"},
            ],
            "collectors": [],
        }

    def test_valid(self):
        self.assertEqual(mod.verify_report(self.valid())["passed"], 2)

    def test_no_tests(self):
        doc = self.valid()
        doc["tests"] = []
        with self.assertRaises(ValueError):
            mod.verify_report(doc)

    def test_forged_duplicate_identity(self):
        doc = self.valid()
        doc["tests"][1]["nodeid"] = doc["tests"][0]["nodeid"]
        with self.assertRaises(ValueError):
            mod.verify_report(doc)

    def test_hidden_skips(self):
        doc = self.valid()
        doc["summary"]["skipped"] = 1
        with self.assertRaises(ValueError):
            mod.verify_report(doc)

    def test_false_green_exit_code(self):
        doc = self.valid()
        doc["exitcode"] = 1
        with self.assertRaises(ValueError):
            mod.verify_report(doc)

    def test_missing_case_with_false_summary(self):
        doc = self.valid()
        doc["tests"].pop()
        with self.assertRaises(ValueError):
            mod.verify_report(doc)


if __name__ == "__main__":
    unittest.main()

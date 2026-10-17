"""Headless identity validation, not native delivery evidence."""
import copy
import unittest

from activation_probe import exact_record

GENERATION = "{7fb07b27-61d4-40fa-bb62-8c2d43d2e50c}"
UUID = "{1ff83344-cc07-42dd-828c-578e58817f43}"


class IdentityValidation(unittest.TestCase):
    def setUp(self):
        self.snapshot = {"generation": GENERATION, "windows": [
            {"pid": 123, "token": 4, "internal_id": UUID, "minimized": False}]}

    def test_exact_target(self):
        self.assertEqual(exact_record(self.snapshot, 123, 4, GENERATION)["internal_id"], UUID)

    def test_same_pid_is_not_identity(self):
        with self.assertRaisesRegex(ValueError, "missing_or_ambiguous"):
            exact_record(self.snapshot, 123, 5, GENERATION)

    def test_reload_rejects_same_token(self):
        with self.assertRaisesRegex(ValueError, "generation_changed"):
            exact_record(self.snapshot, 123, 4, "{56e5794d-1031-4b48-8ae6-1c83da14d6b0}")

    def test_duplicate_tokens_refuse(self):
        self.snapshot["windows"].append(copy.deepcopy(self.snapshot["windows"][0]))
        with self.assertRaisesRegex(ValueError, "ambiguous_tokens"):
            exact_record(self.snapshot, 123, 4, GENERATION)

    def test_recreated_uuid_refuses(self):
        with self.assertRaisesRegex(ValueError, "identity_changed"):
            exact_record(self.snapshot, 123, 4, GENERATION,
                         "{56e5794d-1031-4b48-8ae6-1c83da14d6b0}")

    def test_minimized_refuses(self):
        self.snapshot["windows"][0]["minimized"] = True
        with self.assertRaisesRegex(ValueError, "minimized_target"):
            exact_record(self.snapshot, 123, 4, GENERATION)


if __name__ == "__main__":
    unittest.main()

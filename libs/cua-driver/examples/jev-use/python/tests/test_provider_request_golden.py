from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from provider_request_golden import GOLDEN, encode, legacy_build, legacy_choose, payloads


class ProviderRequestGoldenTest(unittest.TestCase):
    """The provider request and candidate tables match the pre-refactor capture."""

    maxDiff = None

    def assert_golden(self, build, choose) -> None:
        self.assertEqual(encode(payloads(build, choose)), GOLDEN.read_text(encoding="utf-8"))

    def test_core_entry_points_send_byte_identical_requests(self) -> None:
        self.assert_golden(legacy_build, legacy_choose)


if __name__ == "__main__":
    unittest.main()

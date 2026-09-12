from __future__ import annotations

import unittest

from cua_bench_runtime.canon import canonical_json, digest_json


class CanonTests(unittest.TestCase):
    def test_key_order_does_not_change_digest(self) -> None:
        self.assertEqual(digest_json({"b": 2, "a": 1}), digest_json({"a": 1, "b": 2}))
        self.assertEqual(canonical_json({"b": 2, "a": 1}), b'{"a":1,"b":2}')

    def test_unicode_is_encoded_without_ascii_escaping(self) -> None:
        self.assertIn("é".encode(), canonical_json({"text": "é"}))


if __name__ == "__main__":
    unittest.main()

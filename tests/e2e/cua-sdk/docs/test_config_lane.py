"""Unit tests for the config lane's stdlib parts."""

from __future__ import annotations

import unittest

import config_lane as cl


class ConfigLaneTest(unittest.TestCase):
    def test_jsonc(self):
        text = '{\n  // a comment\n  "url": "http://x//y", /* block */\n  "a": [1, 2,],\n}\n'
        self.assertEqual(cl.parse(text, "jsonc", jsonc=True), {"url": "http://x//y", "a": [1, 2]})

    def test_json_and_toml(self):
        self.assertEqual(cl.parse('{"a": 1}', "json"), {"a": 1})
        self.assertEqual(cl.parse('[x]\ny = "z"\n', "toml"), {"x": {"y": "z"}})
        with self.assertRaises(ValueError):
            cl.parse('{"a": 1,}', "json")

    def test_detect_schema(self):
        self.assertEqual(cl.detect_schema({"mcpServers": {}}), "mcp-client-config")
        self.assertEqual(cl.detect_schema({"apiVersion": "images.cua.ai/v1alpha1"}), "cua-image")
        self.assertIsNone(cl.detect_schema([1]))


if __name__ == "__main__":
    unittest.main()

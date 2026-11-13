"""The macOS Space with developer tools in the image catalog: named for the
product, with the published full image as the local-build fallback."""

from __future__ import annotations

import json
import os
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
CATALOG = os.path.join(HERE, "..", "..", "..", "libs", "images", "sandbox-images.json")
MACOS_REPO = "ghcr.io/trycua/" + "macos"


class Catalog(unittest.TestCase):
    def test_catalog_entry_is_named_for_the_product(self):
        with open(CATALOG) as f:
            catalog = json.load(f)
        entries = [e for e in catalog["images"] if e["ref"] == MACOS_REPO + ":26-xcode"]
        self.assertEqual(len(entries), 1)
        e = entries[0]
        # published and its digest go together (the release pipeline records both)
        self.assertEqual(e["published"], "digest" in e)
        self.assertEqual(e["tier"], "xcode")
        self.assertIn("developer tools", e["name"])
        # Built on the user's Mac from the published full image.
        parent = e["derived_from"]
        self.assertEqual(parent, MACOS_REPO + ":26")
        self.assertTrue(any(i["ref"] == parent and i["published"] for i in catalog["images"]))
        self.assertEqual([i["ref"] for i in catalog["images"] if "derived_from" in i], [e["ref"]])


if __name__ == "__main__":
    unittest.main()

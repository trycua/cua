"""Assert the Fleet desktop-control how-to matches the Sandbox Driver API."""

from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[3]
PAGE = (
    ROOT
    / "docs/content/docs/cua-sdk/guides/desktop.mdx"
)
DRIVER = ROOT / "libs/python/cua-sandbox/cua_sandbox/interfaces/driver.py"


class FleetDriverGuideTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.page = PAGE.read_text()

    def test_uses_persistent_language_tabs(self):
        self.assertIn('<Tabs groupId="language" persist items=', self.page)

    def test_driver_connect_matches_the_sdk(self):
        source = DRIVER.read_text()
        self.assertIn("sb.driver.connect()", self.page)
        self.assertRegex(source, r"async def connect\(\s*self,\s*\*,\s*service")
        self.assertIn('service="mcp", transport="mcp"', self.page)

    def test_spacesd_is_the_guest_daemon(self):
        self.assertIn("3211", self.page)
        self.assertIn("SpacesdNotAvailable", self.page)
        self.assertNotRegex(self.page, r"computer-server|:8000|\bport `8000`")
        self.assertNotIn("—", self.page)


if __name__ == "__main__":
    unittest.main()

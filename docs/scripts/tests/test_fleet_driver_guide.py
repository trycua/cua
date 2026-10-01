"""Assert the Fleet Driver how-to preserves the shared service contract."""

from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[3]
PAGE = (
    ROOT
    / "docs/content/docs/how-to-guides/sandbox/control-the-desktop-with-cua-driver.mdx"
)
FLEET_HELPER = ROOT / "libs/cua-driver/typescript/src/fleet.ts"


class FleetDriverGuideTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.page = PAGE.read_text()
        cls.python_tab, cls.typescript_tab = re.findall(
            r'<Tab value="(?:Python|TypeScript)">(.*?)</Tab>', cls.page, re.DOTALL
        )

    def test_uses_persistent_language_tabs(self):
        self.assertIn(
            '<Tabs groupId="language" persist items={[\'Python\', \'TypeScript\']}>',
            self.page,
        )

    def test_both_tabs_resolve_the_advertised_mcp_service(self):
        self.assertIn(
            'sb.driver.connect(service="mcp", transport="mcp")', self.python_tab
        )
        self.assertIn("connectFleetDriver({ client, sandbox })", self.typescript_tab)
        self.assertNotIn("service:", self.typescript_tab)
        self.assertRegex(
            FLEET_HELPER.read_text(),
            r"connectFleetDriver\(\{[\s\S]*?service = 'mcp'",
        )

    def test_shared_requirements_and_cleanup_are_outside_tabs(self):
        before_tabs, after_tabs = self.page.split(
            '<Tabs groupId="language" persist items=', 1
        )
        after_tabs = after_tabs.split("</Tabs>", 1)[1]
        self.assertIn('ai.cua.driver.envelopes', before_tabs)
        self.assertIn("computer-server", before_tabs)
        self.assertIn("close Driver", after_tabs)
        self.assertIn("release the claim", after_tabs)

    def test_does_not_claim_image_support_or_type_script_sandbox_parity(self):
        self.assertIn("No Fleet image is implied to provide this capability", self.page)
        self.assertRegex(self.page, r"not a TypeScript Sandbox\s+SDK")
        self.assertNotRegex(self.page, r"Image\.from_registry|public\.ecr\.aws")


if __name__ == "__main__":
    unittest.main()

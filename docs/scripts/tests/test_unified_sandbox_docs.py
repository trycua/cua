"""One Cua SDK tree: task pages lead with local, which needs no `local=`."""

from pathlib import Path
import json
import unittest


DOCS = Path(__file__).resolve().parents[2] / "content" / "docs"

# Task pages: local is the default, so their examples pass no `local=` and
# show no cloud variant (the cloud has its own guide).
UNIFIED = (
    "cua-sdk/index",
    "cua-sdk/quickstart",
    "cua-sdk/guides/services",
    "cua-sdk/guides/sidecars",
    "cua-sdk/guides/images",
    "cua-sdk/guides/lifecycle",
)
# Hand-written Cua SDK and Cua Fleets pages stay short.
MAX_WORDS = 1000


def page(slug: str) -> str:
    return (DOCS / f"{slug}.mdx").read_text()


class UnifiedSandboxDocsTests(unittest.TestCase):
    def test_task_pages_lead_with_local(self):
        for slug in UNIFIED:
            with self.subTest(slug=slug):
                text = page(slug)
                self.assertNotIn("'Local', 'Cloud'", text, f"{slug} has a cloud tab")
                self.assertNotIn("local=False", text, f"{slug} shows a cloud example")
                self.assertNotRegex(text, r"local=True[,)]", f"{slug} passes the default")

    def test_pages_stay_short(self):
        for folder in ("cua-sdk", "fleets"):
            for path in (DOCS / folder).rglob("*.mdx"):
                if "reference" in path.relative_to(DOCS).parts:
                    continue
                with self.subTest(page=str(path.relative_to(DOCS))):
                    self.assertLess(len(path.read_text().split()), MAX_WORDS)

    def test_happy_path_has_no_pool_vocabulary(self):
        for slug in ("cua-sdk/index", "cua-sdk/quickstart"):
            with self.subTest(slug=slug):
                text = page(slug).lower()
                for word in ("claim_name", "pool_name", "pool.apply", "claim_ttl"):
                    self.assertNotIn(word, text)

    def test_old_image_anchor_still_resolves(self):
        text = page("cua-sdk/guides/images")
        self.assertIn('<span id="choose-a-published-fleet-image" />', text)
        self.assertIn("Windows images do not include a Windows license", text)

    def test_cloud_page_points_to_fleets_for_dedicated_capacity(self):
        text = page("cua-sdk/guides/cloud")
        self.assertIn("local=False", text)
        self.assertIn("(/fleets)", text)
        meta = json.loads((DOCS / "fleets" / "meta.json").read_text())
        self.assertEqual(meta["title"], "Cua Fleets")


if __name__ == "__main__":
    unittest.main()

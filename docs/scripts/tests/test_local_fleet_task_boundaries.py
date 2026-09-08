"""Keep local setup out of the hosted image-selection procedure."""

from pathlib import Path
import re
import unittest


DOCS = Path(__file__).resolve().parents[2] / "content" / "docs"


def page(slug: str) -> str:
    return (DOCS / f"{slug}.mdx").read_text()


class LocalFleetTaskBoundariesTests(unittest.TestCase):
    def test_local_guides_identify_execution_scope(self):
        for slug in ("images", "manage-local-lifecycle", "interactive-shell", "scale-out"):
            with self.subTest(slug=slug):
                text = page(f"how-to-guides/sandbox/{slug}")
                title = re.search(r"^title: (.+)$", text, re.MULTILINE)
                self.assertIsNotNone(title)
                self.assertIn("local", title.group(1).lower())

    def test_fleet_selection_contains_no_local_provisioning(self):
        text = page("how-to-guides/sandbox/choose-an-image")
        self.assertIn("## Choose a published Fleet image", text)
        self.assertIn("Image.from_registry(", text)
        self.assertIn("Windows images do not include a Windows license", text)
        for block in re.findall(r"```python[^\n]*\n(.*?)```", text, re.DOTALL):
            self.assertNotIn("local=True", block)
            self.assertNotIn(".apt_install(", block)
            self.assertNotIn(".run(", block)

    def test_old_image_selection_anchor_forwards_to_hosted_guide(self):
        text = page("how-to-guides/sandbox/images")
        section = text.split("## Choose a published Fleet image\n", 1)[1].split("\n## ", 1)[0]
        self.assertIn(
            "/how-to-guides/sandbox/choose-an-image#choose-a-published-fleet-image", section
        )
        self.assertNotIn("```", section)
        self.assertIn("## Choose a base image", text)
        destination = page("how-to-guides/sandbox/choose-an-image")
        self.assertIn("## Choose a published Fleet image", destination)

    def test_hosted_next_steps_name_the_fleet_image_guide(self):
        for slug in ("concepts/how-fleet-images-work", "tutorials/your-first-cloud-fleet"):
            with self.subTest(slug=slug):
                text = page(slug)
                self.assertIn(
                    "[Choose a Fleet image](/how-to-guides/sandbox/choose-an-image)", text
                )
                self.assertNotIn("[Choose and build a sandbox image]", text)

    def test_shared_reference_exposes_backend_limits(self):
        text = page("reference/sandbox-sdk/index")
        self.assertIn("one API", text)
        self.assertIn("/reference/sandbox-sdk/runtime-support", text)
        self.assertIn("/concepts/sandbox-lifecycle", text)


if __name__ == "__main__":
    unittest.main()

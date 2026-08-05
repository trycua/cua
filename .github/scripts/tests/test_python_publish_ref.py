"""Regression tests for publishing a package from its caller ref."""

from pathlib import Path
import unittest


REPO_ROOT = Path(__file__).resolve().parents[3]
REUSABLE = REPO_ROOT / ".github/workflows/py-reusable-publish.yml"
SANDBOX = REPO_ROOT / ".github/workflows/cd-py-sandbox.yml"


class PythonPublishRefTest(unittest.TestCase):
    def test_sandbox_publishes_from_the_tag_or_dispatched_branch(self) -> None:
        reusable = REUSABLE.read_text()
        sandbox = SANDBOX.read_text()

        self.assertIn("checkout_ref:", reusable)
        self.assertIn('default: "main"', reusable)
        self.assertIn("ref: ${{ inputs.checkout_ref }}", reusable)
        self.assertIn('if [ "$CHECKOUT_REF" = "main" ]; then', reusable)
        self.assertIn("checkout_ref: ${{ github.ref }}", sandbox)


if __name__ == "__main__":
    unittest.main()

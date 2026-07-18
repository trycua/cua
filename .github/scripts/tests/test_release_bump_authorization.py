"""Regression tests for package version-bump authorization."""

from pathlib import Path
import unittest


REPO_ROOT = Path(__file__).resolve().parents[3]


class TestReleaseBumpAuthorization(unittest.TestCase):
    """Verify that humans other than the release owner cannot bump packages."""

    def test_bump_workflow_is_gated_before_write_access(self) -> None:
        workflow = (
            REPO_ROOT / ".github/workflows/release-bump-version.yml"
        ).read_text()

        self.assertIn("permissions: {}", workflow)
        self.assertIn("  authorize:\n", workflow)
        self.assertIn('ACTOR: ${{ github.actor }}', workflow)
        self.assertIn('TRIGGERING_ACTOR: ${{ github.triggering_actor }}', workflow)
        self.assertIn(
            '"f-trycua:f-trycua"|'
            '"cua-release-bot[bot]:cua-release-bot[bot]")',
            workflow,
        )
        self.assertIn("  bump-version:\n    needs: authorize\n", workflow)
        self.assertIn(
            "    permissions:\n      contents: write\n    steps:\n",
            workflow,
        )

    def test_auto_release_requires_owner_applied_labels(self) -> None:
        workflow = (
            REPO_ROOT / ".github/workflows/release-on-merge.yml"
        ).read_text()

        self.assertIn(
            'LABEL_EVENTS=$(gh api --paginate --slurp \\\n'
            '            "repos/$REPO/issues/$PR_NUMBER/events?per_page=100")',
            workflow,
        )
        self.assertIn("label_was_applied_by_release_owner()", workflow)
        self.assertIn('.actor.login == "f-trycua"', workflow)
        self.assertIn(
            'label_was_applied_by_release_owner "bump:major"', workflow
        )
        self.assertIn(
            'label_was_applied_by_release_owner "bump:minor"', workflow
        )
        self.assertIn(
            'label_was_applied_by_release_owner "$release_label"', workflow
        )
        self.assertIn("No owner-authorized release labels found", workflow)


if __name__ == "__main__":
    unittest.main()

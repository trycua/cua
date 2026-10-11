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
        # The gate allows exactly the two release owners (each acting for
        # themselves, never a re-run by someone else) and the release bot.
        authorize_job = workflow.split("  authorize:\n", 1)[1].split(
            "  bump-version:\n", 1
        )[0]
        allowed = [
            line.strip()
            for line in authorize_job.splitlines()
            if line.strip().startswith('"') and line.strip().endswith(")")
        ]
        self.assertEqual(
            allowed,
            [
                '"f-trycua:f-trycua"|'
                '"ddupont808:ddupont808"|'
                '"cua-release-bot[bot]:cua-release-bot[bot]")'
            ],
        )
        self.assertIn('case "$ACTOR:$TRIGGERING_ACTOR" in', authorize_job)
        self.assertIn("            *)\n", authorize_job)
        self.assertIn("              exit 1\n", authorize_job)
        self.assertNotIn("r33drichards", workflow)
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
        # Only the two release owners' label events authorize a release.
        owner_filters = [
            line.strip()
            for line in workflow.splitlines()
            if ".actor.login ==" in line
        ]
        self.assertEqual(
            owner_filters,
            [
                '| .event == "labeled" and (.actor.login == "f-trycua" '
                'or .actor.login == "ddupont808")'
            ],
        )
        self.assertNotIn("r33drichards", workflow)
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

    def test_release_comments_cannot_authorize_or_render_release_checkboxes(
        self,
    ) -> None:
        auto_release = (
            REPO_ROOT / ".github/workflows/release-on-merge.yml"
        ).read_text()
        reminder = (
            REPO_ROOT / ".github/workflows/ci-release-reminder.yml"
        ).read_text()

        auto_release_trigger = auto_release.split("permissions:", 1)[0]
        self.assertIn("pull_request:\n    types: [closed]", auto_release_trigger)
        self.assertNotIn("issue_comment", auto_release_trigger)
        self.assertNotIn("pull_request_review_comment", auto_release_trigger)
        self.assertNotIn("github.event.pull_request.body", auto_release)
        self.assertNotIn("/comments", auto_release)
        self.assertIn(
            "Release authorization comes only from audited label events",
            auto_release,
        )

        reminder_trigger = reminder.split("jobs:", 1)[0]
        self.assertNotIn("issue_comment", reminder_trigger)
        self.assertNotIn("pull_request_review_comment", reminder_trigger)
        self.assertNotIn("- [x]", reminder)
        self.assertNotIn("- [X]", reminder)
        self.assertNotIn("- [ ]", reminder)
        self.assertIn("This comment is status-only", reminder)
        self.assertIn(
            "Only owner-applied \\`release:<service>\\` labels can do that",
            reminder,
        )
        self.assertIn(
            "Ask the release owner to apply \\`release:<service>\\` labels",
            reminder,
        )
        self.assertIn("EXISTING_COMMENT_IDS=$(gh api --paginate", reminder)
        self.assertNotIn("| head -1", reminder)


if __name__ == "__main__":
    unittest.main()

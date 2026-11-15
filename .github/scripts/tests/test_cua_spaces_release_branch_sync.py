"""Regression test: the Cua Spaces release PR is replayed onto main."""

from pathlib import Path
import unittest


REPO_ROOT = Path(__file__).resolve().parents[3]


class TestCuaSpacesReleaseBranchSync(unittest.TestCase):
    """Every release PR rewrites the shared manifest; Spaces must be reconciled too."""

    def test_sync_step_recognizes_spaces(self) -> None:
        workflow = (REPO_ROOT / ".github/workflows/release-please.yml").read_text()
        self.assertIn('.files | any(.path == "apps/cua-spaces-macos/VERSION")', workflow)
        self.assertIn('"$SPACES" != "true"', workflow)
        self.assertIn("COMPONENT_ARGS+=(--component cua-spaces)", workflow)
        self.assertIn("--product spaces", workflow)

    def test_spaces_branch_runs_before_the_sandbox_fallback(self) -> None:
        workflow = (REPO_ROOT / ".github/workflows/release-please.yml").read_text()
        self.assertLess(
            workflow.index('elif [[ "$SPACES" == "true" ]]'),
            workflow.index("--product sandbox"),
        )


if __name__ == "__main__":
    unittest.main()

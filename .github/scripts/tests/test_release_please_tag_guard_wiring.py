"""A merged release PR must tag only its own component (#6350).

release_please_tagging_preview.cjs proves the guard with the pinned engine;
these tests keep the guard wired in front of every place that tags.
"""

from __future__ import annotations

import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
WORKFLOWS = REPO_ROOT / ".github/workflows"
GUARD = ".github/scripts/release_please_tag_guard.cjs"


def step(workflow: str, name: str) -> str:
    text = (WORKFLOWS / workflow).read_text()
    marker = f"- name: {name}\n"
    assert marker in text, f"{workflow}: missing step {name!r}"
    return text.split(marker, 1)[1].split("\n      - ", 1)[0]


class ReleasePleaseTagGuardWiringTest(unittest.TestCase):
    def test_guard_runs_before_release_please_tags(self) -> None:
        text = (WORKFLOWS / "release-please.yml").read_text()
        guard = text.index("- name: Guard merged release PRs against tagging other components")
        tagging = text.index("- name: Tag merged release pull requests")
        self.assertLess(guard, tagging)
        body = step("release-please.yml", "Guard merged release PRs against tagging other components")
        # Same condition as the tagging step, so it can never be skipped alone.
        tagging_if = step("release-please.yml", "Tag merged release pull requests").split("uses:", 1)[0]
        self.assertEqual(body.split("env:", 1)[0].strip(), tagging_if.split("id: release-auto", 1)[1].strip())
        self.assertIn('--label "autorelease: pending"', body)
        self.assertIn(f"GUARD={GUARD}", body)
        self.assertIn('node "$GUARD" check', body)
        self.assertIn('node "$GUARD" repair', body)
        self.assertIn('exit "$FAILED"', body)
        self.assertIn("release-please@17.3.0", body)

    def test_release_prs_are_checked_on_every_edit(self) -> None:
        text = (WORKFLOWS / "ci-release-metadata.yml").read_text()
        self.assertIn("types: [opened, edited, synchronize, reopened", text)
        body = step("ci-release-metadata.yml", "Check that the release PR tags only its own component")
        self.assertIn("release-please--branches--", body)
        self.assertIn(f"GUARD={GUARD}", body)
        self.assertIn('node "$GUARD" check', body)

    def test_ci_runs_the_engine_preview(self) -> None:
        ci = (WORKFLOWS / "ci-test-scripts.yml").read_text()
        self.assertIn("release_please_tagging_preview.cjs", ci)
        self.assertTrue((REPO_ROOT / ".github/scripts/tests/fixtures/release-pr-6350-body.md").is_file())


if __name__ == "__main__":
    unittest.main()
